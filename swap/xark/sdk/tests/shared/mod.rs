use std::panic::{catch_unwind, AssertUnwindSafe};

use spr_hasher::primitives::solana_owner_identity;
use spr_keypair::{constants::BLINDING_LEN, hash::poseidon, NullifierKey};
use spr_program::{
    checked, from_bytes, to_bytes, xark::Input, CircuitError, Field, ProgramProofInputs,
};
use spr_transaction::instructions::transact::PrivateTxHash;
use swap_program::circuits::{Commitment, ProofInputUtxo};
use swap_sdk::proof::{checked_circuit, CircuitId};

/// Nonzero, different trees catch dropped or swapped ids in the commitments.
pub const INPUT_TREE_ID: u16 = 3;
pub const OUTPUT_TREE_ID: u16 = 7;

pub fn fe(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[31] = byte;
    out
}

pub fn field(bytes: &[u8; 32]) -> Field {
    from_bytes(bytes).expect("field element")
}

/// The order authority is a PDA, so its identity carries the Solana owner tag
/// like the program's `PdaOwner` derivation.
pub fn order_utxo_owner_hash(order_authority: &[u8; 32]) -> [u8; 32] {
    let pk_field = solana_owner_identity(order_authority).expect("order authority field");
    let nullifier_pk = NullifierKey::from_secret([0u8; BLINDING_LEN])
        .pubkey()
        .expect("zero-secret nullifier pubkey");
    poseidon(&[&pk_field, &nullifier_pk]).expect("order utxo owner hash")
}

/// A UTXO hash preimage as the swap circuits take it.
pub fn circuit_utxo(utxo: &spr_transaction::utxo::ProofInputUtxo) -> ProofInputUtxo {
    ProofInputUtxo {
        domain: field(&utxo.domain),
        tree_id: field(&utxo.tree_id),
        owner_hash: field(&utxo.owner_hash),
        asset: field(&utxo.asset),
        amount: field(&utxo.amount),
        blinding: field(&utxo.blinding),
        data_hash: field(&utxo.data_hash),
        ring_data_hash: field(&utxo.ring_data_hash),
        ring_program_id: field(&utxo.ring_program_id),
    }
}

fn utxo_hash(utxo: &ProofInputUtxo) -> [u8; 32] {
    to_bytes(&utxo.hash().expect("utxo hash"))
}

/// SPP's private transaction hash over circuit UTXOs; `None` is a padding
/// slot.
pub fn private_tx_hash(
    inputs: &[Option<&ProofInputUtxo>],
    outputs: &[&ProofInputUtxo],
    blinding: &Field,
) -> Field {
    let inputs: Vec<[u8; 32]> = inputs
        .iter()
        .map(|input| input.map_or([0u8; 32], utxo_hash))
        .collect();
    let outputs: Vec<[u8; 32]> = outputs.iter().map(|output| utxo_hash(output)).collect();
    field(
        &PrivateTxHash::new(&inputs, &outputs, &to_bytes(blinding))
            .hash()
            .expect("private tx hash"),
    )
}

/// The native circuit and the compiled circuit both reject `program`. The
/// compiled check fails inside `execute`, before the outer public-hash check.
pub fn assert_invalid<P: Input>(
    id: CircuitId,
    program: &P,
    execute: impl FnOnce() -> Result<Commitment, CircuitError>,
) {
    let native = catch_unwind(AssertUnwindSafe(|| checked(execute)));
    assert!(
        !matches!(native, Ok(Ok(_))),
        "the native circuit accepted invalid inputs"
    );
    let (_private, circuit) = checked_circuit(id).expect("checked circuit");
    let error = circuit
        .check(ProgramProofInputs::new(program, Field::zero()))
        .expect_err("the compiled circuit must reject invalid inputs");
    // A constraint inside `execute` names it as the failing function, or as
    // the entry line that calls it when the constraint is inlined there.
    let error = error.to_string();
    assert!(
        error.contains("in execute") || error.contains("program.execute()"),
        "expected the program's constraint to fail before the outer public-hash check: {error}"
    );
}

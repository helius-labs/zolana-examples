use solana_address::Address;
use solana_keypair::Keypair;
use spr_hasher::primitives::solana_owner_identity;
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_program::{
    circuit::CheckedTransaction, from_bytes, to_bytes, Field, InputRecords, ProgramProofInputs,
    ProgramTransaction, TxContext,
};
use spr_transaction::{
    instructions::transact::FinalizedTransaction, utxo::Utxo, Data, Mint, WalletUtxo,
};
use std::ops::{Deref, DerefMut};
use timelock_escrow_program::circuits as circuit;
use timelock_escrow_sdk::{
    escrow_authority,
    instructions::escrow::ESCROW_SLOT,
    proof::CircuitId,
    state::{escrow_utxo, EscrowTerms},
};

/// The tree the spent UTXOs are in. Non-zero, and different from the output
/// tree, so a swapped or dropped tree id changes the commitments the proof
/// binds.
pub const INPUT_TREE_ID: u16 = 3;
pub const OUTPUT_TREE_ID: u16 = 7;
pub const UNLOCK: u64 = 1_700_000_000;

pub trait Execute: spr_program::xark::Input {
    fn execute(&self) -> Result<CheckedTransaction, spr_program::CircuitError>;
}

macro_rules! program {
    ($($name:ident),*) => {$(
        impl Execute for circuit::$name {
            fn execute(&self) -> Result<CheckedTransaction, spr_program::CircuitError> {
                circuit::$name::execute(self)
            }
        }
        pub type $name = Fixture<circuit::$name>;
    )*};
}
program!(Escrow, Withdraw);

/// A program's inputs and the native records they were converted from.
pub struct Fixture<P> {
    pub program: P,
    records: InputRecords,
    /// The creator: the transaction's sender.
    pub creator: ShieldedKeypair,
}

impl<P> Deref for Fixture<P> {
    type Target = P;
    fn deref(&self) -> &P {
        &self.program
    }
}

impl<P> DerefMut for Fixture<P> {
    fn deref_mut(&mut self) -> &mut P {
        &mut self.program
    }
}

impl<P: Execute> Fixture<P> {
    pub fn sender(&self) -> ShieldedAddress {
        self.creator.shielded_address().unwrap()
    }

    pub fn create_program_transaction(&self) -> anyhow::Result<ProgramTransaction> {
        Ok(self.records.create_program_transaction(
            &self.program,
            self.execute()?,
            &self.sender(),
            Address::default(),
        )?)
    }

    pub fn create_finalized_transaction(&self) -> anyhow::Result<FinalizedTransaction> {
        Ok(self.create_program_transaction()?.finalized)
    }
}

pub fn creator() -> ShieldedKeypair {
    ShieldedKeypair::from_keypair(&Keypair::new()).unwrap()
}

/// The escrow terms the fixtures commit for `creator`.
pub fn terms(creator: &ShieldedKeypair) -> EscrowTerms {
    let address = creator.shielded_address().unwrap();
    EscrowTerms {
        creator: *InputRecords::new().address(&address).unwrap().owner(),
        unlock: UNLOCK,
    }
}

pub fn mint() -> Mint {
    Mint::new(Address::new_from_array([3; 32]), 2)
}

pub fn fe(value: u8) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[31] = value;
    bytes
}

pub fn field(value: u8) -> Field {
    Field::from(value)
}

pub fn field_bytes(value: Field) -> [u8; 32] {
    to_bytes(&value)
}

pub fn context(seed: u8) -> TxContext {
    InputRecords::context(&fe(seed), Some(OUTPUT_TREE_ID)).unwrap()
}

/// The creator's Solana pubkey.
pub fn creator_address(creator: &ShieldedKeypair) -> Address {
    creator
        .shielded_address()
        .unwrap()
        .solana_address()
        .unwrap()
}

/// The address of `creator`'s escrow UTXOs: its escrow-authority PDA, with
/// its own viewing key.
pub fn escrow_owner(creator: &ShieldedKeypair) -> ShieldedAddress {
    escrow_authority(&creator_address(creator))
        .address(creator.shielded_address().unwrap().viewing_pubkey)
        .unwrap()
}

/// `hash_bytes` of the creator's tagged Solana pubkey.
pub fn owner_pk_field(creator: &ShieldedKeypair) -> Field {
    let address = creator
        .shielded_address()
        .unwrap()
        .solana_address()
        .unwrap();
    from_bytes(&solana_owner_identity(address.as_array()).unwrap()).unwrap()
}

/// A plain `amount` UTXO of [`mint`] owned by `owner`, at leaf 0 of
/// [`OUTPUT_TREE_ID`].
pub fn token_input(owner: &ShieldedKeypair, amount: u64) -> WalletUtxo {
    spr_test_utils::utxo::wallet(
        Utxo {
            owner: owner.signing_pubkey(),
            asset: mint(),
            amount,
            blinding: fe(5),
            ring_program_id: None,
            data: Data::default(),
        },
        &owner.nullifier_key,
        OUTPUT_TREE_ID,
        0,
        None,
        None,
    )
    .unwrap()
}

/// An escrow of `escrow_amount` from a source UTXO of the creator holding
/// `escrow_amount + change_amount`.
pub fn escrow_inputs(escrow_amount: u64, change_amount: u64) -> Escrow {
    escrow_inputs_of(creator(), escrow_amount, change_amount)
}

fn escrow_inputs_of(creator: ShieldedKeypair, escrow_amount: u64, change_amount: u64) -> Escrow {
    let escrow_owner = escrow_owner(&creator);
    escrow_inputs_owned_by(creator, escrow_amount, change_amount, &escrow_owner)
}

/// An escrow of `escrow_amount` from a source UTXO of the creator holding
/// `escrow_amount + change_amount`, whose escrow UTXO goes to `escrow_owner`.
pub fn escrow_inputs_owned_by(
    creator: ShieldedKeypair,
    escrow_amount: u64,
    change_amount: u64,
    escrow_owner: &ShieldedAddress,
) -> Escrow {
    let source = token_input(&creator, escrow_amount + change_amount);
    let mut records = InputRecords::new();
    let private = circuit::EscrowPrivateInputs {
        tx_context: context(41),
        source: records.utxo(&source).unwrap(),
        amount: escrow_amount,
        unlock: UNLOCK,
    };
    let public = circuit::EscrowPublicInputs {
        owner_pk_field: owner_pk_field(&creator),
        escrow_owner: records.address(escrow_owner).unwrap(),
    };
    Fixture {
        program: circuit::Escrow { private, public },
        records,
        creator,
    }
}

/// A withdraw of the 1 000-unit escrow UTXO the escrow fixture outputs, at
/// leaf 5 of [`INPUT_TREE_ID`].
pub fn withdraw_inputs() -> Withdraw {
    let escrow = escrow_inputs_of(creator(), 1_000, 0);
    let output = escrow
        .create_finalized_transaction()
        .unwrap()
        .output_utxos()
        .get(ESCROW_SLOT)
        .unwrap()
        .clone();
    let creator = escrow.creator;
    let escrow_utxo = escrow_utxo(&creator_address(&creator), &output, INPUT_TREE_ID, 5).unwrap();
    let mut records = InputRecords::new();
    let private = circuit::WithdrawPrivateInputs {
        tx_context: context(42),
        escrow: records.utxo(&escrow_utxo.utxo).unwrap(),
        terms: escrow_utxo.data,
    };
    let public = circuit::WithdrawPublicInputs {
        unlock: UNLOCK,
        owner_pk_field: owner_pk_field(&creator),
    };
    Fixture {
        program: circuit::Withdraw { private, public },
        records,
        creator,
    }
}

/// Proves `inputs` with the checked circuit, verifies the proof against `vk`
/// with the transaction's public hash, and checks a tampered public hash
/// fails. Returns the program transaction and the proof.
pub fn prove_and_verify<P: Execute>(
    id: CircuitId,
    inputs: &Fixture<P>,
    vk: &groth16_solana::groth16::Groth16Verifyingkey,
) -> (ProgramTransaction, spr_program::CompressedProof) {
    let tx = inputs.create_program_transaction().unwrap();
    let (_private, circuit) = timelock_escrow_sdk::proof::checked_circuit(id).unwrap();
    circuit.check_constrained(tx.proof_inputs.clone()).unwrap();
    let proof = timelock_escrow_sdk::proof::prove(id, &tx.proof_inputs).unwrap();
    assert!(
        proof.a.iter().any(|&byte| byte != 0),
        "proof_a must not be all zero"
    );
    let a = groth16_solana::decompression::decompress_g1(&proof.a).unwrap();
    let b = groth16_solana::decompression::decompress_g2(&proof.b).unwrap();
    let c = groth16_solana::decompression::decompress_g1(&proof.c).unwrap();
    groth16_solana::groth16::Groth16Verifier::new(&a, &b, &c, &[tx.public_hash], vk)
        .unwrap()
        .verify()
        .unwrap();
    let mut tampered = tx.public_hash;
    tampered[31] ^= 1;
    assert!(
        groth16_solana::groth16::Groth16Verifier::new(&a, &b, &c, &[tampered], vk)
            .unwrap()
            .verify()
            .is_err(),
        "verification must fail for a tampered public input"
    );
    (tx, proof)
}

/// Checks that both the native execution and the compiled circuit refuse
/// `inputs`.
pub fn assert_invalid<P: Execute>(id: CircuitId, inputs: &Fixture<P>) {
    let native = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        spr_program::checked(|| inputs.execute())
    }));
    assert!(
        !matches!(native, Ok(Ok(_))),
        "the native circuit accepted invalid inputs"
    );
    let (_private, circuit) = timelock_escrow_sdk::proof::checked_circuit(id).unwrap();
    let error = circuit
        .check(ProgramProofInputs::new(&inputs.program, Field::zero()))
        .expect_err("the compiled circuit must reject invalid inputs");
    assert!(
        error.to_string().contains("program.execute()"),
        "expected the program's constraint to fail before the outer public-hash check: {error}"
    );
}

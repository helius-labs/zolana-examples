use solana_address::Address;
use spr_hasher::primitives::hash_bytes;
use spr_keypair::{hash::poseidon, ViewingKey};
use spr_program::{to_bytes, CompressedProof, Field};
use spr_transaction::utxo::ProofInputUtxo;
use swap_program::{
    circuits::{Cancel, CancelPrivateInputs, CancelPublicInputs, OrderTerms},
    verifier::cancel::{PublicInputs, Verifier, VERIFYINGKEY},
};
use swap_sdk::{proof::CircuitId, prover::SwapProverClient};

use crate::shared::{
    assert_invalid, circuit_utxo, fe, field, order_utxo_owner_hash, private_tx_hash, INPUT_TREE_ID,
    OUTPUT_TREE_ID,
};

const MAKER_OWNER_PK_FIELD: u8 = 71;
const MAKER_NULLIFIER_PK: u8 = 88;

fn maker_owner_hash() -> [u8; 32] {
    poseidon(&[&fe(MAKER_OWNER_PK_FIELD), &fe(MAKER_NULLIFIER_PK)]).expect("owner hash")
}

/// A cancel refunding the order to `source_output_owner`.
fn build_inputs(source_output_owner: [u8; 32]) -> Cancel {
    let order = OrderTerms {
        destination_asset: field(&hash_bytes(&[2u8; 32]).expect("destination asset")),
        destination_amount: 250,
        maker_owner_hash: field(&maker_owner_hash()),
        maker_viewing_pk: *ViewingKey::new().pubkey().as_bytes(),
        expiry: 1_700_000_000,
        taker_pk_fe: field(&fe(123)),
    };
    let source_mint = Address::new_from_array([1u8; 32]);
    let order_utxo = ProofInputUtxo::new(
        order_utxo_owner_hash(&fe(42)),
        &source_mint,
        1_000,
        &fe(7),
        INPUT_TREE_ID,
    )
    .expect("order utxo")
    .with_data_hash(to_bytes(&order.data_hash().expect("order data hash")));
    let source_output = ProofInputUtxo::new(
        source_output_owner,
        &source_mint,
        1_000,
        &fe(11),
        OUTPUT_TREE_ID,
    )
    .expect("source output utxo");
    let mut cancel = Cancel {
        public: CancelPublicInputs {
            expiry: order.expiry,
            maker_owner_pk_field: field(&fe(MAKER_OWNER_PK_FIELD)),
        },
        private: CancelPrivateInputs {
            private_tx_hash: Field::zero(),
            order,
            order_utxo: circuit_utxo(&order_utxo),
            source_output: circuit_utxo(&source_output),
            maker_nullifier_pk: field(&fe(MAKER_NULLIFIER_PK)),
            private_tx_blinding: field(&fe(21)),
        },
    };
    refresh_hash(&mut cancel);
    cancel
}

fn sample_inputs() -> Cancel {
    build_inputs(maker_owner_hash())
}

/// Recompute the private transaction hash after a mutation. The compact
/// padding output does not enter it.
fn refresh_hash(cancel: &mut Cancel) {
    let private = &cancel.private;
    cancel.private.private_tx_hash = private_tx_hash(
        &[Some(&private.order_utxo)],
        &[&private.source_output],
        &private.private_tx_blinding,
    );
}

/// The program's verifier with the order expiry, the signer identity and the
/// transact's private transaction hash.
fn verify(
    proof: &CompressedProof,
    expiry: u64,
    maker_owner_pk_field: [u8; 32],
    private_tx_hash: [u8; 32],
) -> bool {
    Verifier {
        proof,
        public_inputs: PublicInputs {
            expiry,
            maker_owner_pk_field,
        },
        private_tx_hash: &private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
}

#[test]
fn program_vk_has_one_public_input() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn cancel_prove_verify_and_bind_public_inputs() {
    // 1. Prove a cancel.
    let cancel = sample_inputs();
    let proof = SwapProverClient::new()
        .prove_cancel(&cancel)
        .expect("prove");
    assert!(proof.a.iter().any(|byte| *byte != 0));

    // 2. The committed verifier accepts the instruction's values.
    let expiry = cancel.public.expiry;
    let maker = fe(MAKER_OWNER_PK_FIELD);
    let tx_hash = to_bytes(&cancel.private.private_tx_hash);
    assert!(verify(&proof, expiry, maker, tx_hash));

    // 3. It rejects another expiry, signer or transaction.
    let mut tampered = tx_hash;
    tampered[31] ^= 0x01;
    assert!(!verify(&proof, expiry - 1, maker, tx_hash));
    assert!(!verify(&proof, expiry, fe(72), tx_hash));
    assert!(!verify(&proof, expiry, maker, tampered));
}

#[test]
fn cancel_rejects_wrong_source_output_owner() {
    let mut wrong_owner = maker_owner_hash();
    if let Some(last) = wrong_owner.last_mut() {
        *last ^= 0x01;
    }
    let cancel = build_inputs(wrong_owner);
    assert_invalid(CircuitId::Cancel, &cancel, || cancel.execute());
}

#[test]
fn cancel_rejects_wrong_signer_expiry_and_refund() {
    // Tamper one signer, expiry or refund constraint at a time.
    let mutations: [fn(&mut Cancel); 6] = [
        |cancel| cancel.public.maker_owner_pk_field = field(&fe(72)),
        |cancel| cancel.private.maker_nullifier_pk = field(&fe(89)),
        |cancel| cancel.public.expiry -= 1,
        |cancel| cancel.private.source_output.amount = field(&fe(1)),
        |cancel| cancel.private.source_output.asset = field(&fe(1)),
        |cancel| cancel.private.order_utxo.data_hash = field(&fe(1)),
    ];
    for mutate in mutations {
        let mut cancel = sample_inputs();
        mutate(&mut cancel);
        refresh_hash(&mut cancel);
        assert_invalid(CircuitId::Cancel, &cancel, || cancel.execute());
    }
}

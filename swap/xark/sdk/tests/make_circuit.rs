use solana_address::Address;
use spr_hasher::primitives::hash_bytes;
use spr_keypair::ViewingKey;
use spr_program::{to_bytes, Field};
use spr_transaction::{instructions::transact::PrivateTxHash, utxo::ProofInputUtxo};
use swap_program::{
    circuits::{Make, MakePrivateInputs, MakePublicInputs, OrderTerms},
    verifier::make::{PublicInputs, Verifier, VERIFYINGKEY},
};
use swap_sdk::{proof::CircuitId, prover::SwapProverClient};

use crate::shared::{
    assert_invalid, circuit_utxo, fe, field, order_utxo_owner_hash, OUTPUT_TREE_ID,
};

fn sample_order(destination_amount: u64) -> OrderTerms {
    OrderTerms {
        destination_asset: field(&hash_bytes(&[2u8; 32]).expect("destination asset")),
        destination_amount,
        maker_owner_hash: field(&fe(99)),
        maker_viewing_pk: *ViewingKey::new().pubkey().as_bytes(),
        expiry: 1_700_000_000,
        taker_pk_fe: field(&fe(123)),
    }
}

/// A make of `order` with a change output of `change_amount`. Both outputs
/// land in the output tree.
fn build_inputs(order: OrderTerms, change_amount: u64) -> Make {
    let source_mint = Address::new_from_array([1u8; 32]);
    let order_utxo = ProofInputUtxo::new(
        order_utxo_owner_hash(&fe(42)),
        &source_mint,
        1_000,
        &fe(7),
        OUTPUT_TREE_ID,
    )
    .expect("order utxo")
    .with_data_hash(to_bytes(&order.data_hash().expect("order data hash")));
    let change = ProofInputUtxo::new(
        to_bytes(&order.maker_owner_hash),
        &source_mint,
        change_amount,
        &fe(6),
        OUTPUT_TREE_ID,
    )
    .expect("change utxo");
    let mut make = Make {
        private: MakePrivateInputs {
            private_tx_hash: Field::zero(),
            order,
            order_utxo: circuit_utxo(&order_utxo),
            change: circuit_utxo(&change),
            source_input_hash: field(&fe(5)),
            private_tx_blinding: field(&fe(21)),
        },
        public: MakePublicInputs {},
    };
    refresh_hash(&mut make);
    make
}

/// Recompute the private transaction hash after a mutation, so a negative
/// fails on the mutated constraint rather than on a stale hash.
fn refresh_hash(make: &mut Make) {
    let private = &make.private;
    let hash = PrivateTxHash::new(
        &[to_bytes(&private.source_input_hash), [0u8; 32]],
        &[
            to_bytes(&private.change.hash().expect("change hash")),
            to_bytes(&private.order_utxo.hash().expect("order hash")),
        ],
        &to_bytes(&private.private_tx_blinding),
    )
    .hash()
    .expect("private tx hash");
    make.private.private_tx_hash = field(&hash);
}

fn verify(make: &Make, proof: &spr_program::CompressedProof, private_tx_hash: [u8; 32]) -> bool {
    Verifier {
        proof,
        public_inputs: PublicInputs {},
        private_tx_hash: &private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
        && make.private.private_tx_hash == field(&private_tx_hash)
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
fn make_prove_verify() {
    // 1. Prove a make with a nonzero change.
    let make = build_inputs(sample_order(250), 750);
    let proof = SwapProverClient::new().prove_make(&make).expect("prove");
    assert!(proof.a.iter().any(|byte| *byte != 0));

    // 2. The committed verifier accepts it for the transaction it commits and
    // rejects any other transaction.
    let private_tx_hash = to_bytes(&make.private.private_tx_hash);
    assert!(verify(&make, &proof, private_tx_hash));
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    assert!(Verifier {
        proof: &proof,
        public_inputs: PublicInputs {},
        private_tx_hash: &tampered,
        transfers: &[],
    }
    .verify()
    .is_err());
}

#[test]
fn make_zero_change_proves() {
    // A zero-value change output is a real output: its hash enters
    // private_tx_hash, matching SPP.
    let make = build_inputs(sample_order(250), 0);
    let proof = SwapProverClient::new().prove_make(&make).expect("prove");
    assert!(verify(
        &make,
        &proof,
        to_bytes(&make.private.private_tx_hash)
    ));
}

#[test]
fn make_rejects_invalid_orders() {
    // 1. A zero destination amount.
    let make = build_inputs(sample_order(0), 750);
    assert_invalid(CircuitId::Make, &make, || make.execute());

    // 2. Tamper one order UTXO or change constraint at a time.
    let mutations: [fn(&mut Make); 7] = [
        |make| make.private.order.expiry += 1,
        |make| make.private.order_utxo.amount = Field::zero(),
        |make| make.private.order_utxo.ring_program_id = field(&fe(1)),
        |make| make.private.change.asset = field(&fe(9)),
        |make| make.private.change.owner_hash = field(&fe(98)),
        |make| make.private.change.data_hash = field(&fe(1)),
        |make| make.private.order_utxo.domain = field(&fe(1)),
    ];
    for mutate in mutations {
        let mut make = build_inputs(sample_order(250), 750);
        mutate(&mut make);
        refresh_hash(&mut make);
        assert_invalid(CircuitId::Make, &make, || make.execute());
    }

    // 3. A private transaction hash for another transaction.
    let mut make = build_inputs(sample_order(250), 750);
    make.private.private_tx_hash = field(&fe(17));
    assert_invalid(CircuitId::Make, &make, || make.execute());
}

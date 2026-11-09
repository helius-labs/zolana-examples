use crate::shared::*;
use swap_program::verifier::make::{PublicInputs, Verifier, VERIFYINGKEY};
use swap_sdk::{
    instructions::make::{CHANGE_SLOT, ORDER_SLOT},
    proof::CircuitId,
};

const CIRCUIT: CircuitId = CircuitId::Make;

#[test]
fn program_vk_has_one_public_input() {
    assert_eq!(VERIFYINGKEY.nr_pubinputs, 1);
    assert!(VERIFYINGKEY.vk_commitment.is_none());
    assert_eq!(
        VERIFYINGKEY.vk_ic.len(),
        2,
        "standard Groth16 vk_ic length must be public_inputs + 1"
    );
}

#[test]
fn make_prove_verify() {
    // 1. Check the change returns to the maker and the order UTXO commits the
    // terms with their payload.
    let inputs = make_inputs(SOURCE_AMOUNT, 750);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let outputs = finalized.output_utxos();
    let change = outputs.get(CHANGE_SLOT).unwrap();
    let order = outputs.get(ORDER_SLOT).unwrap();
    let terms = &inputs.program.private.terms;
    assert_eq!(
        (
            change.owner_address,
            change.asset,
            change.amount,
            change.data_hash
        ),
        (Some(inputs.sender_address()), source_mint(), 750, None)
    );
    let terms_hash = spr_program::circuit::DataHash::hash(terms).unwrap();
    assert_eq!(
        (
            order.owner_address.unwrap().owner_hash().unwrap(),
            order.asset,
            order.amount,
            order.data_hash,
        ),
        (
            swap_sdk::order_authority().owner_hash().unwrap(),
            source_mint(),
            SOURCE_AMOUNT,
            Some(spr_program::hasher::data_hash(&field_bytes(terms_hash)).unwrap()),
        )
    );

    // 2. Prove, and verify with the program's verifier for the transaction it
    // commits; any other transaction fails.
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
    assert!(verify(&proof, &private_tx_hash));
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    assert!(!verify(&proof, &tampered));
}

#[test]
fn make_zero_change_proves() {
    // A zero change is still a real output before the order UTXO.
    let inputs = make_inputs(SOURCE_AMOUNT, 0);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let change = finalized.output_utxos().get(CHANGE_SLOT).unwrap();
    assert_eq!((change.amount, change.compact), (0, false));
    prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
}

#[test]
fn make_rejects_invalid_orders() {
    // A zero order or destination amount, or an order beyond the source.
    let mutations: [fn(&mut Make); 3] = [
        |inputs| inputs.program.private.terms.destination_amount = 0,
        |inputs| inputs.program.private.source_amount = 0,
        |inputs| inputs.program.private.source_amount = SOURCE_AMOUNT + 751,
    ];
    for mutate in mutations {
        let mut inputs = make_inputs(SOURCE_AMOUNT, 750);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

fn verify(proof: &spr_program::CompressedProof, private_tx_hash: &[u8; 32]) -> bool {
    Verifier {
        proof,
        public_inputs: PublicInputs {},
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
}

use crate::shared::*;
use spr_program::CompressedProof;
use swap_program::verifier::take::{PublicInputs, Verifier, VERIFYINGKEY};
use swap_sdk::{
    instructions::take::{take_blinding_seed, TakeTransactionParams, DESTINATION_OUTPUT_SLOT},
    proof::CircuitId,
};

const CIRCUIT: CircuitId = CircuitId::Take;

/// A take of an order designated to `taker` (or open with `designated:
/// false`), sent by `taker`.
fn inputs(designated: bool) -> Take {
    let taker = keypair();
    let order = order_utxo(terms(&keypair(), designated.then(|| identity_of(&taker))));
    take_inputs(&order, taker)
}

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
fn designated_take_proves_and_binds_public_inputs() {
    // 1. Check the source goes to the taker and the destination to the maker.
    let inputs = inputs(true);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let terms = &inputs.program.private.terms;
    let outputs = finalized.output_utxos();
    let source = outputs.first().unwrap();
    let destination = outputs.get(DESTINATION_OUTPUT_SLOT).unwrap();
    assert_eq!(
        (
            source.owner_address,
            source.asset,
            source.amount,
            destination.asset,
            destination.amount,
        ),
        (
            Some(inputs.sender_address()),
            source_mint(),
            SOURCE_AMOUNT,
            destination_mint(),
            DESTINATION_AMOUNT,
        )
    );
    assert_eq!(
        spr_program::to_bytes(&terms.maker.owner().hash().unwrap()),
        destination.owner_address.unwrap().owner_hash().unwrap()
    );

    // 2. Prove, and verify with the program's verifier from the transact's
    // expiry, first nullifier and private transaction hash.
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
    let first_nullifier = finalized.first_nullifier().unwrap();
    assert_eq!(
        first_nullifier,
        field_bytes(inputs.program.public.first_nullifier)
    );
    assert!(verify(&proof, EXPIRY, first_nullifier, &private_tx_hash));

    // 3. Another expiry, first nullifier or transaction fails.
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    assert!(!verify(
        &proof,
        EXPIRY + 1,
        first_nullifier,
        &private_tx_hash
    ));
    assert!(!verify(&proof, EXPIRY, fe(17), &private_tx_hash));
    assert!(!verify(&proof, EXPIRY, first_nullifier, &tampered));
}

#[test]
fn open_order_is_taken_by_an_arbitrary_taker() {
    // `taker_pk_fe == 0` accepts any taker input owner.
    let inputs = inputs(false);
    assert_eq!(inputs.program.private.terms.taker_pk_fe, field(0));
    prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
}

#[test]
fn designated_order_rejects_another_taker() {
    // 1. Designate one taker and take with another.
    let order = order_utxo(terms(&keypair(), Some(identity_of(&keypair()))));
    let inputs = take_inputs(&order, keypair());
    // 2. Native execution and the compiled circuit both refuse it.
    assert_invalid(CIRCUIT, &inputs);
    // 3. The SDK refuses to build it.
    let taker = inputs.sender;
    let error = TakeTransactionParams {
        order_utxo: order,
        order_leaf_index: ORDER_LEAF_INDEX,
        taker_input: wallet_utxo(&taker, destination_mint(), DESTINATION_AMOUNT),
        payer: solana_address::Address::default(),
        tree_id: INPUT_TREE_ID,
    }
    .build(&taker)
    .err()
    .expect("another taker must be refused");
    assert!(error.to_string().contains("designates another taker"));
}

#[test]
fn take_rejects_an_unrecoverable_blinding_seed_and_first_nullifier() {
    // A seed other than the order's, or a first nullifier other than the
    // published one, would hide the maker payout from the order opening.
    let mutations: [fn(&mut Take); 3] = [
        |inputs| inputs.program.private.tx_context = context(&fe(29)),
        |inputs| inputs.program.public.first_nullifier = field(17),
        |inputs| inputs.program.private.order_utxo.blinding = field(29),
    ];
    for mutate in mutations {
        let mut inputs = inputs(true);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn take_rejects_wrong_terms_and_taker_input() {
    // Tamper one terms, order UTXO or taker input constraint at a time.
    let mutations: [fn(&mut Take); 7] = [
        |inputs| inputs.program.public.expiry = EXPIRY + 1,
        |inputs| inputs.program.private.terms.destination_amount = 0,
        |inputs| inputs.program.private.terms.destination_amount = DESTINATION_AMOUNT - 1,
        |inputs| inputs.program.private.terms.destination_mint = [9; 32],
        |inputs| {
            let other = keypair().shielded_address().unwrap();
            inputs.program.private.terms.maker = inputs.address(other);
        },
        |inputs| inputs.program.private.order_utxo.data_hash = field(99),
        |inputs| inputs.program.private.taker_in.amount = DESTINATION_AMOUNT + 1,
    ];
    for mutate in mutations {
        let mut inputs = inputs(true);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn maker_recovers_payout_without_settlement_ciphertext() {
    // 1. Build the take through the SDK.
    let taker = keypair();
    let order = order_utxo(terms(&keypair(), Some(identity_of(&taker))));
    let transaction = TakeTransactionParams {
        order_utxo: order.clone(),
        order_leaf_index: ORDER_LEAF_INDEX,
        taker_input: wallet_utxo(&taker, destination_mint(), DESTINATION_AMOUNT),
        payer: solana_address::Address::default(),
        tree_id: INPUT_TREE_ID,
    }
    .build(&taker)
    .unwrap();
    assert_eq!(
        transaction.spp_proof_inputs.external_data.expiry_unix_ts,
        EXPIRY
    );

    // 2. The maker derives the payout from the order opening and the first
    // nullifier alone.
    let payout = transaction
        .spp_proof_inputs
        .external_data
        .outputs
        .get(DESTINATION_OUTPUT_SLOT)
        .unwrap()
        .utxo_hash;
    let recovered = order
        .derived_destination_output(&transaction.first_nullifier)
        .unwrap();
    assert_eq!(recovered.hash(INPUT_TREE_ID).unwrap(), payout);
    assert_eq!(recovered.blinding, transaction.destination_output_blinding);
    assert_eq!(
        take_blinding_seed(&order.blinding).unwrap(),
        transaction.spp_proof_inputs.blinding_seed
    );

    // 3. Another first nullifier derives another payout.
    let wrong = order.derived_destination_output(&fe(17)).unwrap();
    assert_ne!(wrong.hash(INPUT_TREE_ID).unwrap(), payout);
}

fn verify(
    proof: &CompressedProof,
    expiry: u64,
    first_nullifier: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> bool {
    Verifier {
        proof,
        public_inputs: PublicInputs {
            expiry,
            first_nullifier,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
}

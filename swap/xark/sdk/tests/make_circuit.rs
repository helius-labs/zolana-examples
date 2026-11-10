use crate::shared::*;
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use swap_program::{
    instructions::shared::OrderAuthority,
    verifier::make::{PublicInputs, Verifier, VERIFYINGKEY},
};
use swap_sdk::{
    instructions::make::{CHANGE_SLOT, ORDER_SLOT},
    order_authority, order_authority_bump,
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
    // 1. Check the change returns to the maker, the source owner, and the
    // order UTXO commits the terms with the maker as their maker.
    let mut inputs = make_inputs(SOURCE_AMOUNT, 750);
    let terms = make_terms(&mut inputs);
    let finalized = inputs.create_finalized_transaction().unwrap();
    let outputs = finalized.output_utxos();
    let change = outputs.get(CHANGE_SLOT).unwrap();
    let order = outputs.get(ORDER_SLOT).unwrap();
    assert_eq!(
        (
            change.owner_address,
            change.asset,
            change.amount,
            change.data_hash
        ),
        (Some(inputs.sender_address()), source_mint(), 750, None)
    );
    let terms_hash = spr_program::circuit::DataHash::hash(&terms).unwrap();
    let maker_pubkey = inputs.sender_address().solana_address().unwrap();
    assert_eq!(
        (
            order.owner_address.unwrap().owner_hash().unwrap(),
            order.asset,
            order.amount,
            order.data_hash,
        ),
        (
            order_authority(&maker_pubkey).owner_hash().unwrap(),
            source_mint(),
            SOURCE_AMOUNT,
            Some(spr_program::hasher::data_hash(&field_bytes(terms_hash)).unwrap()),
        )
    );

    // 2. Prove, and verify with the program's public inputs for the maker
    // signer and the order's viewing key, and the transaction it commits;
    // another signer, viewing key or transaction fails.
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
    let viewing_pubkey = *inputs.public.order_owner.viewing_pubkey();
    let program_verify = |signer: &ShieldedAddress, viewing_pubkey, tx_hash| {
        verify(
            &proof,
            program_public_inputs(signer, viewing_pubkey),
            tx_hash,
        )
    };
    let signer = inputs.sender_address();
    assert!(program_verify(&signer, &viewing_pubkey, &private_tx_hash));
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    assert!(!program_verify(&signer, &viewing_pubkey, &tampered));
    let other = keypair().shielded_address().unwrap();
    assert!(!program_verify(&other, &viewing_pubkey, &private_tx_hash));
    assert!(!program_verify(
        &signer,
        other.viewing_pubkey.as_bytes(),
        &private_tx_hash
    ));
}

/// SPP accepts a data-bearing output owned by any owner signer, so a maker
/// can make an order UTXO it owns itself, or one under another maker's order
/// authority, and prove the make for that owner. The proof is valid for its
/// own `order_owner`, but the program derives `order_owner` from the maker
/// signer's order authority, and the proof fails against it.
#[test]
fn make_of_an_order_not_owned_by_the_makers_order_authority_is_rejected() {
    let taker = keypair();
    let owners: [fn(&Make, &ShieldedKeypair) -> ShieldedAddress; 2] = [
        // The maker itself, with the taker's viewing key.
        |inputs, taker| ShieldedAddress {
            viewing_pubkey: taker.viewing_pubkey(),
            ..inputs.sender_address()
        },
        // The maker's order authority, under another maker's.
        |_, taker| order_address(&keypair(), taker),
    ];
    for owner in owners {
        let mut inputs = make_inputs(SOURCE_AMOUNT, 750);
        let order_owner = owner(&inputs, &taker);
        inputs.public.order_owner = inputs.address(order_owner);

        // 1. The order output is owned by `order_owner`, and the proof is
        // valid for it.
        let finalized = inputs.create_finalized_transaction().unwrap();
        let order = finalized.output_utxos().get(ORDER_SLOT).unwrap().clone();
        assert_eq!(order.owner_address, Some(order_owner));
        let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
        let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
        let viewing_pubkey = order_owner.viewing_pubkey.as_bytes();
        let signer = inputs.sender_address();
        let claimed = PublicInputs {
            order_owner: spr_program::verifier::address_hash(
                &order_owner.owner_hash().unwrap(),
                viewing_pubkey,
            )
            .unwrap(),
            ..program_public_inputs(&signer, viewing_pubkey)
        };
        assert!(verify(&proof, claimed, &private_tx_hash));

        // 2. The program's public inputs reject it.
        assert!(!verify(
            &proof,
            program_public_inputs(&signer, viewing_pubkey),
            &private_tx_hash
        ));
    }
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
    // A zero order or destination amount, an order beyond the source, or a
    // signer other than the source owner.
    let mutations: [fn(&mut Make); 4] = [
        |inputs| inputs.program.private.destination_amount = 0,
        |inputs| inputs.program.private.source_amount = 0,
        |inputs| inputs.program.private.source_amount = SOURCE_AMOUNT + 751,
        |inputs| inputs.program.public.maker_owner_pk_field = field(89),
    ];
    for mutate in mutations {
        let mut inputs = make_inputs(SOURCE_AMOUNT, 750);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

/// The public inputs the program computes for the maker signer `signer` and
/// the order's `viewing_pubkey` from the make arguments.
fn program_public_inputs(signer: &ShieldedAddress, viewing_pubkey: &[u8; 33]) -> PublicInputs {
    let maker = signer.solana_address().unwrap();
    OrderAuthority::new(&maker.to_bytes(), order_authority_bump(&maker))
        .unwrap()
        .make_public_inputs(viewing_pubkey)
        .unwrap()
}

fn verify(
    proof: &spr_program::CompressedProof,
    public_inputs: PublicInputs,
    private_tx_hash: &[u8; 32],
) -> bool {
    Verifier {
        proof,
        public_inputs,
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
}

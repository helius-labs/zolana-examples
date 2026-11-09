use crate::shared::*;
use swap_program::verifier::cancel::{PublicInputs, Verifier, VERIFYINGKEY};
use swap_sdk::{instructions::cancel::SOURCE_OUTPUT_SLOT, proof::CircuitId};

const CIRCUIT: CircuitId = CircuitId::Cancel;

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
fn cancel_prove_verify_and_bind_public_inputs() {
    // 1. Check the whole order returns to the maker, with compact output
    // padding.
    let inputs = cancel_inputs();
    let finalized = inputs.create_finalized_transaction().unwrap();
    let refund = finalized.output_utxos().get(SOURCE_OUTPUT_SLOT).unwrap();
    assert_eq!(
        (
            refund.owner_address,
            refund.asset,
            refund.amount,
            refund.data_hash,
            finalized.output_utxos().last().unwrap().compact,
        ),
        (
            Some(inputs.sender_address()),
            source_mint(),
            SOURCE_AMOUNT,
            None,
            true
        )
    );

    // 2. Prove, and verify with the program's verifier from the order expiry
    // and the maker signer.
    let (_, proof) = prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    let private_tx_hash = finalized.padding_independent_private_tx_hash().unwrap();
    let signer = identity_of(&inputs.sender);
    assert!(verify(&proof, EXPIRY, signer, &private_tx_hash));

    // 3. Another expiry, signer or transaction fails.
    let mut tampered = private_tx_hash;
    tampered[31] ^= 0x01;
    assert!(!verify(&proof, EXPIRY + 1, signer, &private_tx_hash));
    assert!(!verify(
        &proof,
        EXPIRY,
        identity_of(&keypair()),
        &private_tx_hash
    ));
    assert!(!verify(&proof, EXPIRY, signer, &tampered));
}

#[test]
fn cancel_rejects_wrong_signer_expiry_maker_and_order() {
    // Tamper one signer, expiry, refund owner or order constraint at a time.
    let mutations: [fn(&mut Cancel); 5] = [
        |inputs| inputs.program.public.expiry = EXPIRY + 1,
        |inputs| inputs.program.public.maker_owner_pk_field = field(89),
        |inputs| {
            let other = keypair().shielded_address().unwrap();
            inputs.program.private.terms.maker = inputs.address(other);
        },
        |inputs| inputs.program.private.order_utxo.data_hash = field(99),
        |inputs| inputs.program.private.order_utxo.amount = 0,
    ];
    for mutate in mutations {
        let mut inputs = cancel_inputs();
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

fn verify(
    proof: &spr_program::CompressedProof,
    expiry: u64,
    maker_owner_pk_field: [u8; 32],
    private_tx_hash: &[u8; 32],
) -> bool {
    Verifier {
        proof,
        public_inputs: PublicInputs {
            expiry,
            maker_owner_pk_field,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
    .is_ok()
}

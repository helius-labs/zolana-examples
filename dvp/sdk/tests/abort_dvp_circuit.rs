use crate::shared::*;
use dvp_program::{circuits::AbortDvp, verifier::abort_dvp::VERIFYINGKEY};
use dvp_sdk::proof::CircuitId;
use zolana_program::ZkProgram;

const CIRCUIT: CircuitId = CircuitId::AbortDvp;

#[test]
fn cancel_and_reject_refund_every_funding_combination() {
    for (reject, buyer) in [(false, false), (true, false), (true, true)] {
        for (delivery, payment) in [(false, false), (true, false), (false, true), (true, true)] {
            // 1. Build the authorized abort with either optional deposit.
            let inputs = abort_inputs(reject, buyer, delivery, payment);
            let finalized = inputs
                .create_finalized_transaction(&destination(91), sample_terms().creator)
                .unwrap();
            // 2. Check real deposits precede padding and both refunds remain real.
            let expected_inputs = match (delivery, payment) {
                (true, true) => vec![false, false, false],
                (false, false) => vec![false, true, true],
                _ => vec![false, false, true],
            };
            assert_eq!(
                finalized
                    .input_utxos()
                    .iter()
                    .map(|utxo| utxo.is_dummy())
                    .collect::<Vec<_>>(),
                expected_inputs
            );
            let terms = sample_terms();
            assert_eq!(
                finalized
                    .output_utxos()
                    .iter()
                    .map(|utxo| (
                        utxo.owner_address.unwrap(),
                        utxo.asset,
                        utxo.amount,
                        utxo.compact
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (
                        terms.seller_refund_destination,
                        delivery_mint(),
                        if delivery { 1_017 } else { 0 },
                        false
                    ),
                    (
                        terms.buyer_refund_destination,
                        zolana_transaction::Mint::SOL,
                        if payment { 2_499_999_995 } else { 0 },
                        false
                    ),
                ]
            );
            // 3. Prove the abort, including a payment-only deposit before padding.
            prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
        }
    }
}

#[test]
fn abort_rejects_unauthorized_signers_and_wrong_deposit_state() {
    // 1. Tamper authorization, state, seed, owner, or mint.
    let mutations: [fn(&mut AbortDvp); 10] = [
        |inputs| inputs.public.signer_identity = fe(99),
        |inputs| inputs.public.signer_identity = identity(sample_terms().seller),
        |inputs| inputs.public.creator_identity = fe(99),
        |inputs| inputs.public.dvp_id = fe(99),
        |inputs| inputs.private.tx_context.blinding_seed = fe(99),
        |inputs| inputs.private.dvp_utxo.data_hash = Some(fe(99)),
        |inputs| inputs.private.delivery_deposit.utxo.asset = zolana_transaction::Mint::SOL,
        |inputs| inputs.private.payment_deposit.utxo.owner = destination(91).signing_pubkey,
        |inputs| inputs.private.delivery_deposit.data_hash = Some(fe(99)),
        |inputs| inputs.private.terms.seller_refund_destination.address = destination(91),
    ];
    // 2. Check every mutation is rejected.
    for mutate in mutations {
        let mut inputs = abort_inputs(false, false, true, true);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
    // 3. Check reject accepts only the selected party.
    let mut inputs = abort_inputs(true, false, true, true);
    inputs.public.signer_identity = identity(sample_terms().settlement_authority);
    assert_invalid(CIRCUIT, &inputs);
}

#[test]
fn abort_proof_rejects_wrong_refunds_and_spp_rejects_padding_before_a_deposit() {
    // 1. Prove a payment-only abort with semantic refund order.
    let inputs = abort_inputs(false, false, false, true);
    let finalized = inputs
        .create_finalized_transaction(&destination(91), sample_terms().creator)
        .unwrap();
    let proof = dvp_sdk::proof::prove(CIRCUIT, &inputs).unwrap();
    let public = dvp_program::verifier::abort_dvp::PublicInputs {
        creator_identity: inputs.public.creator_identity,
        dvp_id: inputs.public.dvp_id,
        dvp_owner: inputs.public.dvp_owner.owner_hash().unwrap(),
        signer_identity: inputs.public.signer_identity,
        reject: false,
    };
    // 2. Recompute the SPP hash after tampering each refund or input order.
    for mutation in 0..2 {
        let input_hashes = vec![
            inputs.private.dvp_utxo.utxo_hash,
            inputs.private.payment_deposit.utxo_hash,
            [0; 32],
        ];
        let mut outputs = finalized.output_utxos().to_vec();
        match mutation {
            0 => outputs.get_mut(1).unwrap().amount -= 1,
            1 => outputs.first_mut().unwrap().owner_address = Some(destination(91)),
            _ => unreachable!(),
        }
        let hashes = outputs
            .iter()
            .map(|output| output.hash(OUTPUT_TREE_ID).unwrap())
            .collect::<Vec<_>>();
        let hash = zolana_program::PrivateTxHash::new(
            &input_hashes,
            &hashes,
            &finalized.private_tx_blinding().unwrap(),
        )
        .hash()
        .unwrap();
        assert!(dvp_program::verifier::abort_dvp::verify(&proof, &public, &hash).is_err());
    }
    // 3. Check SPP rejects a real deposit after a dummy.
    let mut spp = finalized
        .encrypt_with_viewing_key(&zolana_keypair::ViewingKey::new())
        .unwrap();
    spp.input_utxos.swap(1, 2);
    assert!(matches!(
        spp.check_shape(),
        Err(zolana_transaction::TransactionError::RealInputAfterDummy { index: 2 })
    ));
}

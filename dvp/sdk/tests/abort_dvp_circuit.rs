use crate::shared::*;
use dvp_program::verifier::abort_dvp::VERIFYINGKEY;
use dvp_sdk::proof::CircuitId;

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
            // 2. Check only funded deposits are included and both refunds remain real.
            let expected_inputs = vec![false; 1 + usize::from(delivery) + usize::from(payment)];
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
            // 3. Prove the abort for this funding combination.
            prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
        }
    }
}

#[test]
fn abort_rejects_unauthorized_signers_and_wrong_deposit_state() {
    // 1. Tamper authorization, state, seed, owner, or mint.
    let mutations: [fn(&mut AbortDvp); 10] = [
        |inputs| inputs.public.signer_identity = field(99),
        |inputs| inputs.public.signer_identity = field_identity(sample_terms().seller),
        |inputs| inputs.public.creator_identity = field(99),
        |inputs| inputs.public.dvp_id = field(99),
        |inputs| inputs.private.tx_context.blinding_seed = field(99),
        |inputs| inputs.private.dvp_utxo.data_hash = field(99),
        |inputs| inputs.private.delivery_deposit.asset = [0; 32],
        |inputs| inputs.private.payment_deposit.owner = *inputs.address(destination(91)).owner(),
        |inputs| inputs.private.delivery_deposit.data_hash = field(99),
        |inputs| {
            inputs.private.terms.seller_refund_destination.address = inputs.address(destination(91))
        },
    ];
    // 2. Check every mutation is rejected.
    for mutate in mutations {
        let mut inputs = abort_inputs(false, false, true, true);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
    // 3. Check reject accepts only the selected party.
    let mut inputs = abort_inputs(true, false, true, true);
    inputs.public.signer_identity = field_identity(sample_terms().settlement_authority);
    assert_invalid(CIRCUIT, &inputs);
}

#[test]
fn abort_proof_rejects_wrong_refunds_and_spp_rejects_padding_before_a_deposit() {
    // 1. Prove a payment-only abort with semantic refund order.
    let inputs = abort_inputs(false, false, false, true);
    let finalized = inputs
        .create_finalized_transaction(&destination(91), sample_terms().creator)
        .unwrap();
    let proof = dvp_sdk::proof::prove(CIRCUIT, &inputs.proof_inputs()).unwrap();
    let public = dvp_program::verifier::abort_dvp::PublicInputs {
        creator_identity: field_bytes(inputs.public.creator_identity),
        dvp_id: field_bytes(inputs.public.dvp_id),
        dvp_owner: field_bytes(inputs.public.dvp_owner.owner().hash().unwrap()),
        signer_identity: field_bytes(inputs.public.signer_identity),
        reject: false,
    };
    // 2. Recompute the SPP hash after tampering each refund.
    for mutation in 0..2 {
        let input_hashes = vec![
            field_bytes(inputs.private.dvp_utxo.hash().unwrap()),
            field_bytes(inputs.private.payment_deposit.hash().unwrap()),
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
        assert!(verify(&proof, &public, &hash).is_err());
    }
    // 3. Check SPP rejects a real deposit after a dummy.
    let mut spp = finalized
        .encrypt_with_viewing_key(&zolana_keypair::ViewingKey::new())
        .unwrap();
    spp.input_utxos.insert(
        1,
        zolana_transaction::utxo::SppProofInputUtxo::dummy(OUTPUT_TREE_ID).unwrap(),
    );
    assert!(matches!(
        spp.check_shape(),
        Err(zolana_transaction::TransactionError::RealInputAfterDummy { index: 2 })
    ));
}

fn verify(
    proof: &zolana_program::CompressedProof,
    public: &dvp_program::verifier::abort_dvp::PublicInputs,
    private_tx_hash: &[u8; 32],
) -> Result<(), zolana_program::verifier::VerifyError> {
    dvp_program::verifier::abort_dvp::Verifier {
        proof,
        public_inputs: dvp_program::verifier::abort_dvp::PublicInputs {
            creator_identity: public.creator_identity,
            dvp_id: public.dvp_id,
            dvp_owner: public.dvp_owner,
            signer_identity: public.signer_identity,
            reject: public.reject,
        },
        private_tx_hash,
        transfers: &[],
    }
    .verify()
}

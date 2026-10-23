use crate::shared::*;
use dvp_program::{circuits::ReclaimDvp, verifier::reclaim_dvp::VERIFYINGKEY};
use dvp_sdk::proof::CircuitId;
use zolana_program::ZkProgramCircuit;

const CIRCUIT: CircuitId = CircuitId::ReclaimDvp;

#[test]
fn each_party_reclaims_its_leg_to_a_chosen_destination() {
    for payment in [false, true] {
        // 1. Choose a refund destination outside the committed terms.
        let mut inputs = reclaim_inputs(payment);
        inputs.private.refund_destination = destination(91);
        let finalized = inputs
            .create_finalized_transaction(&destination(91), sample_terms().creator)
            .unwrap();
        // 2. Check the full deposit is refunded with compact output padding.
        let refund = finalized.output_utxos().first().unwrap();
        assert_eq!(
            (
                refund.owner_address,
                refund.asset,
                refund.amount,
                refund.compact
            ),
            (
                Some(destination(91)),
                inputs.private.deposit.utxo.asset,
                inputs.private.deposit.utxo.amount,
                false
            )
        );
        assert!(finalized.output_utxos().last().unwrap().compact);
        // 3. Prove the reclaim and reject a tampered public hash.
        prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    }
}

#[test]
fn reclaim_rejects_wrong_party_leg_owner_mint_and_data() {
    // 1. Tamper one signer, owner, mint, or data constraint.
    let mutations: [fn(&mut ReclaimDvp); 8] = [
        |inputs| inputs.public.signer_identity = fe(99),
        |inputs| inputs.private.payment_leg = true,
        |inputs| inputs.public.creator_identity = fe(99),
        |inputs| inputs.public.dvp_id = fe(99),
        |inputs| inputs.private.deposit.data_hash = Some(fe(99)),
        |inputs| inputs.private.deposit.utxo.owner = destination(91).signing_pubkey,
        |inputs| inputs.private.deposit.utxo.asset = zolana_transaction::Mint::SOL,
        |inputs| inputs.private.terms.blinding = fe(99),
    ];
    // 2. Check each mutation is unprovable.
    for mutate in mutations {
        let mut inputs = reclaim_inputs(false);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn reclaim_proof_rejects_partial_or_wrong_asset_refunds() {
    // 1. Prove the full refund.
    let inputs = reclaim_inputs(false);
    let finalized = inputs
        .create_finalized_transaction(&destination(91), sample_terms().creator)
        .unwrap();
    let proof = dvp_sdk::proof::prove(CIRCUIT, &inputs).unwrap();
    let public = dvp_program::verifier::reclaim_dvp::PublicInputs {
        creator_identity: inputs.public.creator_identity,
        dvp_id: inputs.public.dvp_id,
        dvp_owner: inputs.public.dvp_owner.owner_hash().unwrap(),
        signer_identity: inputs.public.signer_identity,
    };
    // 2. Recompute the SPP hash for each unauthorized refund.
    for wrong_asset in [false, true] {
        let mut output = finalized.output_utxos().first().unwrap().clone();
        if wrong_asset {
            output.asset = zolana_transaction::Mint::SOL;
        } else {
            output.amount -= 1;
        }
        let hash = zolana_program::PrivateTxHash::new(
            &[inputs.private.deposit.utxo_hash],
            &[output.hash(OUTPUT_TREE_ID).unwrap(), [0; 32]],
            &finalized.private_tx_blinding().unwrap(),
        )
        .hash()
        .unwrap();
        assert!(dvp_program::verifier::reclaim_dvp::Verifier {
            proof: &proof,
            public_inputs: dvp_program::verifier::reclaim_dvp::PublicInputs { ..public },
            private_tx_hash: &hash,
            transfers: &[]
        }
        .verify()
        .is_err());
    }
}

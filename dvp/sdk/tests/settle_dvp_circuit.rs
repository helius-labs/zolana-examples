use crate::shared::*;
use dvp_program::{circuits::SettleDvp, verifier::settle_dvp::VERIFYINGKEY};
use dvp_sdk::proof::CircuitId;
use zolana_program::ZkProgram;

const CIRCUIT: CircuitId = CircuitId::SettleDvp;

#[test]
fn settle_pays_both_legs_and_preserves_real_zero_change() {
    for surplus in [[0, 0], [17, 23]] {
        // 1. Build the settlement with exact funding or surplus.
        let inputs = settle_inputs(surplus);
        let finalized = inputs
            .create_finalized_transaction(&destination(91), sample_terms().creator)
            .unwrap();
        // 2. Check all four destinations, mints, and amounts.
        let terms = sample_terms();
        let expected = [
            (
                terms.buyer_settlement_destination,
                delivery_mint(),
                terms.delivery_amount,
            ),
            (
                terms.seller_settlement_destination,
                zolana_transaction::Mint::SOL,
                terms.payment_amount,
            ),
            (terms.seller_refund_destination, delivery_mint(), surplus[0]),
            (
                terms.buyer_refund_destination,
                zolana_transaction::Mint::SOL,
                surplus[1],
            ),
        ];
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
            expected.map(|(owner, mint, amount)| (owner, mint, amount, false))
        );
        // 3. Prove the atomic settlement.
        prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    }
}

#[test]
fn settle_accepts_both_time_bounds_and_optional_lower_bound() {
    // 1. Check inclusive earliest and expiry timestamps.
    for timestamp in [1_700_000_000, 1_800_000_000] {
        let mut inputs = settle_inputs([0, 0]);
        inputs.public.current_timestamp = timestamp;
        prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
    }
    // 2. Remove the optional lower bound and recompute the committed state.
    let mut terms = sample_terms();
    terms.earliest_settlement_timestamp = None;
    let mut inputs = settle_inputs([0, 0]);
    inputs.private.terms = terms
        .circuit_terms(delivery_mint(), zolana_transaction::Mint::SOL)
        .unwrap();
    inputs.public.dvp_id = terms.dvp_id().unwrap();
    inputs.public.dvp_owner = owner(&terms);
    inputs.private.dvp_utxo = dvp_sdk::state::DvpUtxo::new(terms.clone(), 3)
        .to_input_utxo(2)
        .unwrap();
    inputs.private.delivery_deposit =
        deposit(&terms, dvp_sdk::state::Leg::Delivery, terms.delivery_amount)
            .to_input_utxo()
            .unwrap();
    inputs.private.payment_deposit =
        deposit(&terms, dvp_sdk::state::Leg::Payment, terms.payment_amount)
            .to_input_utxo()
            .unwrap();
    prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
}

#[test]
fn settle_rejects_wrong_authority_time_commitments_and_deposits() {
    // 1. Tamper one authorization, time, state, or deposit constraint.
    let mutations: [fn(&mut SettleDvp); 15] = [
        |inputs| inputs.public.settlement_authority_identity = fe(99),
        |inputs| inputs.public.current_timestamp = 1_800_000_001,
        |inputs| inputs.public.current_timestamp = 1_699_999_999,
        |inputs| inputs.public.current_timestamp = u64::MAX,
        |inputs| inputs.public.creator_identity = fe(99),
        |inputs| inputs.public.dvp_id = fe(99),
        |inputs| inputs.private.tx_context.blinding_seed = fe(99),
        |inputs| inputs.private.delivery_deposit.utxo.amount -= 1,
        |inputs| inputs.private.payment_deposit.utxo.amount -= 1,
        |inputs| {
            std::mem::swap(
                &mut inputs.private.delivery_deposit,
                &mut inputs.private.payment_deposit,
            )
        },
        |inputs| inputs.private.delivery_deposit.utxo.owner = destination(91).signing_pubkey,
        |inputs| inputs.private.payment_deposit.data_hash = Some(fe(99)),
        |inputs| inputs.private.dvp_utxo.utxo.amount = 1,
        |inputs| inputs.private.terms.buyer_settlement_destination.address = destination(91),
        |inputs| inputs.private.terms.seller_refund_destination.address = destination(91),
    ];
    // 2. Check every tampered transaction fails before publication.
    for mutate in mutations {
        let mut inputs = settle_inputs([0, 0]);
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn settle_program_proof_rejects_modified_outputs() {
    // 1. Prove the authorized settlement.
    let inputs = settle_inputs([17, 23]);
    let finalized = inputs
        .create_finalized_transaction(&destination(91), sample_terms().creator)
        .unwrap();
    let proof = dvp_sdk::proof::prove(CIRCUIT, &inputs).unwrap();
    let public = dvp_program::verifier::settle_dvp::PublicInputs {
        creator_identity: inputs.public.creator_identity,
        dvp_id: inputs.public.dvp_id,
        dvp_owner: inputs.public.dvp_owner.owner_hash().unwrap(),
        settlement_authority_identity: inputs.public.settlement_authority_identity,
        current_timestamp: inputs.public.current_timestamp,
    };
    // 2. Change a settlement amount, destination, mint, and derived blinding.
    for mutation in 0..4 {
        let mut outputs = finalized.output_utxos().to_vec();
        let output = outputs.first_mut().unwrap();
        match mutation {
            0 => output.amount += 1,
            1 => output.owner_address = Some(destination(91)),
            2 => output.asset = zolana_transaction::Mint::SOL,
            3 => output.blinding = fe(99),
            _ => unreachable!(),
        }
        let hashes = outputs
            .iter()
            .map(|output| output.hash(OUTPUT_TREE_ID).unwrap())
            .collect::<Vec<_>>();
        let private_hash = zolana_program::PrivateTxHash::new(
            &finalized
                .input_utxos()
                .iter()
                .map(|input| input.utxo_hash)
                .collect::<Vec<_>>(),
            &hashes,
            &finalized.private_tx_blinding().unwrap(),
        )
        .hash()
        .unwrap();
        assert!(dvp_program::verifier::settle_dvp::verify(&proof, &public, &private_hash).is_err());
    }
}

#[test]
fn settlement_proof_rejects_a_different_state_commitment_or_first_nullifier() {
    // 1. Prove settlement of the created state.
    let inputs = settle_inputs([0, 0]);
    let proof = dvp_sdk::proof::prove(CIRCUIT, &inputs).unwrap();
    let public = dvp_program::verifier::settle_dvp::PublicInputs {
        creator_identity: inputs.public.creator_identity,
        dvp_id: inputs.public.dvp_id,
        dvp_owner: inputs.public.dvp_owner.owner_hash().unwrap(),
        settlement_authority_identity: inputs.public.settlement_authority_identity,
        current_timestamp: inputs.public.current_timestamp,
    };
    for change_blinding in [false, true] {
        // 2. Build a different transaction, preserving valid state fields.
        let mut altered = inputs.clone();
        if change_blinding {
            let state = &mut altered.private.dvp_utxo;
            state.utxo.blinding = fe(99);
            state.utxo_hash = state
                .utxo
                .hash(
                    &state.nullifier_pubkey,
                    &state.data_hash.unwrap_or_default(),
                    &state.ring_data_hash.unwrap_or_default(),
                    state.tree_id,
                )
                .unwrap();
            state.nullifier = state
                .utxo
                .nullifier(
                    &state.utxo_hash,
                    &zolana_program::ProgramOwner::nullifier_key(),
                )
                .unwrap();
        } else {
            altered.private.dvp_utxo.nullifier = fe(99);
        }
        let transaction = altered
            .create_finalized_transaction(&destination(91), sample_terms().creator)
            .unwrap();
        // 3. Check the original proof cannot authorize it.
        assert!(dvp_program::verifier::settle_dvp::verify(
            &proof,
            &public,
            &transaction.padding_independent_private_tx_hash().unwrap()
        )
        .is_err());
    }
}

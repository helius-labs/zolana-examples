use crate::shared::*;
use dvp_program::{circuits::CreateDvp, verifier::create_dvp::VERIFYINGKEY};
use dvp_sdk::{instructions::create_dvp::CreateDvpDerivation, proof::CircuitId};
use zolana_program::ZkProgramCircuit;

const CIRCUIT: CircuitId = CircuitId::CreateDvp;

#[test]
fn create_reserves_the_address_and_commits_the_spec_state() {
    // 1. Build the program transaction.
    let terms = sample_terms();
    let inputs = create_inputs(&terms);
    let finalized = inputs
        .create_finalized_transaction(&destination(91), terms.creator)
        .unwrap();
    // 2. Check the state and address commitments independently.
    let derivation = CreateDvpDerivation::new(&terms, OUTPUT_TREE_ID).unwrap();
    let input = finalized.input_utxos().first().unwrap();
    let output = finalized.output_utxos().first().unwrap();
    assert_eq!(
        (
            input.is_address(),
            input.nullifier,
            output.hash(OUTPUT_TREE_ID).unwrap(),
            output.data_hash,
            output.amount
        ),
        (
            true,
            derivation.address,
            derivation.dvp_utxo_hash,
            Some(terms.dvp_id().unwrap()),
            0
        )
    );
    assert_eq!(
        finalized.padding_independent_private_tx_hash().unwrap(),
        derivation.private_tx_hash
    );
    // 3. Verify the proof and reject a tampered public hash.
    prove_and_verify(CIRCUIT, &inputs, &VERIFYINGKEY);
}

#[test]
fn create_without_earliest_settlement_proves() {
    // 1. Omit the optional lower time bound.
    let mut terms = sample_terms();
    terms.earliest_settlement_timestamp = None;
    // 2. Prove the transaction.
    prove_and_verify(CIRCUIT, &create_inputs(&terms), &VERIFYINGKEY);
}

#[test]
fn create_rejects_invalid_trade_terms() {
    // 1. Keep each mutation internally committed to its new DvP ID.
    let mutations: [fn(&mut dvp_sdk::state::SwapDvp); 4] = [
        |terms| terms.delivery_amount = 0,
        |terms| terms.payment_amount = 0,
        |terms| terms.payment_mint = terms.delivery_mint,
        |terms| terms.earliest_settlement_timestamp = Some(terms.expiry_timestamp + 1),
    ];
    // 2. Check the circuit rejects each invalid trade.
    for mutate in mutations {
        let mut terms = sample_terms();
        mutate(&mut terms);
        assert_invalid(CIRCUIT, &create_inputs(&terms));
    }
}

#[test]
fn create_rejects_wrong_commitments_and_seed() {
    // 1. Tamper one constraint at a time.
    let mutations: [fn(&mut CreateDvp); 6] = [
        |inputs| inputs.public.creator_identity = fe(99),
        |inputs| inputs.public.dvp_id = fe(99),
        |inputs| inputs.private.tx_context.blinding_seed = fe(99),
        |inputs| inputs.private.terms.blinding = fe(99),
        |inputs| inputs.private.terms.earliest_settlement_timestamp = u64::MAX,
        |inputs| {
            inputs.private.terms.has_earliest_settlement = false;
        },
    ];
    // 2. Check the native builder and prover reject each mutation.
    for mutate in mutations {
        let mut inputs = create_inputs(&sample_terms());
        mutate(&mut inputs);
        assert_invalid(CIRCUIT, &inputs);
    }
}

#[test]
fn create_rejects_non_pda_address_owners() {
    // 1. Replace the program owner with an ordinary private wallet.
    let mut inputs = create_inputs(&sample_terms());
    inputs.public.dvp_owner = destination(91);
    // 2. Check the address reservation rejects it.
    assert_invalid(CIRCUIT, &inputs);
}

#[test]
fn create_proof_rejects_changed_state_owner_tree_data_and_blinding() {
    // 1. Prove the committed create transaction.
    let terms = sample_terms();
    let inputs = create_inputs(&terms);
    let finalized = inputs
        .create_finalized_transaction(&destination(91), terms.creator)
        .unwrap();
    let proof = dvp_sdk::proof::prove(CIRCUIT, &inputs).unwrap();
    let public = dvp_program::verifier::create_dvp::PublicInputs {
        creator_identity: inputs.public.creator_identity,
        dvp_id: inputs.public.dvp_id,
        dvp_owner: inputs.public.dvp_owner.owner_hash().unwrap(),
    };
    let address = finalized.first_nullifier().unwrap();
    // 2. Recompute the SPP commitment after changing each state field.
    for mutation in 0..4 {
        let mut output = finalized.output_utxos().first().unwrap().clone();
        let mut tree = OUTPUT_TREE_ID;
        match mutation {
            0 => output.owner_address = Some(destination(91)),
            1 => tree += 1,
            2 => output.data_hash = Some(fe(99)),
            3 => output.blinding = fe(99),
            _ => unreachable!(),
        }
        let private_hash = zolana_program::PrivateTxHash {
            input_hashes: &[[0; 32]],
            output_hashes: &[output.hash(tree).unwrap(), [0; 32]],
            address_nullifiers: Some(&[address]),
            blinding: &finalized.private_tx_blinding().unwrap(),
        }
        .hash()
        .unwrap();
        assert!(dvp_program::verifier::create_dvp::Verifier {
            proof: &proof,
            public_inputs: dvp_program::verifier::create_dvp::PublicInputs { ..public },
            private_tx_hash: &private_hash,
            transfers: &[]
        }
        .verify()
        .is_err());
    }
    // 3. Check a different public owner cannot use the proof.
    let wrong_owner = dvp_program::verifier::create_dvp::PublicInputs {
        dvp_owner: destination(91).owner_hash().unwrap(),
        ..public
    };
    assert!(dvp_program::verifier::create_dvp::Verifier {
        proof: &proof,
        public_inputs: dvp_program::verifier::create_dvp::PublicInputs { ..wrong_owner },
        private_tx_hash: &finalized.padding_independent_private_tx_hash().unwrap(),
        transfers: &[]
    }
    .verify()
    .is_err());
}

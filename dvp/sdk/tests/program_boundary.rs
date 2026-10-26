use std::{env, path::Path};

use dvp_program::error::DvpError;
use dvp_sdk::{
    instructions::{
        cancel_dvp::CancelDvp, create_dvp::CreateDvp, reclaim_dvp::ReclaimDvp,
        reject_dvp::RejectDvp, settle_dvp::SettleDvp,
    },
    proof::CircuitId,
};
use mollusk_svm::{result::Check, Mollusk};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use zolana_interface::instruction::instruction_data::transact::{
    CircuitId as SppCircuitId, TransactIxData, TransactProof,
};
use zolana_program::{CompressedProof, ZkProgramCircuit};

use crate::shared::*;

fn proof_and_spp<P: ZkProgramCircuit>(
    circuit: CircuitId,
    inputs: &P,
) -> (CompressedProof, TransactIxData) {
    let finalized = inputs
        .create_finalized_transaction(&destination(91), sample_terms().creator)
        .unwrap();
    let proof = dvp_sdk::proof::prove(circuit, inputs).unwrap();
    let spp = TransactIxData {
        expiry_unix_ts: u64::MAX,
        private_tx_hash: finalized.padding_independent_private_tx_hash().unwrap(),
        circuit: SppCircuitId::ConfidentialEddsa(1, 2, zolana_interface::N_PUBLIC_SLOTS as u8),
        tx_viewing_pk: [2; 33],
        salt: [3; 16],
        proof: TransactProof::zeroed(),
        inputs: vec![],
        interface_transfers: vec![],
        data_hash: None,
        ring_data_hash: None,
        outputs: vec![],
        messages: vec![],
        tree_contexts: vec![],
    };
    (proof, spp)
}

fn assert_program_boundary(viewing_pubkey: [u8; 33], build: impl Fn([u8; 33]) -> Instruction) {
    env::set_var(
        "SBF_OUT_DIR",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/deploy"),
    );
    let mut mollusk = Mollusk::new(&dvp_program::ID, "dvp_program");
    mollusk.compute_budget.compute_unit_limit = 1_400_000;
    mollusk.sysvars.clock.unix_timestamp = NOW as i64;
    for (key, expected) in [
        (viewing_pubkey, ProgramError::NotEnoughAccountKeys),
        (
            *destination(92).viewing_pubkey.as_bytes(),
            ProgramError::Custom(DvpError::ProofVerificationFailed as u32),
        ),
    ] {
        let mut instruction = build(key);
        // Stop after proof verification, before CPI: only one forwarded SPP account remains.
        instruction.accounts.truncate(3);
        let accounts: Vec<_> = instruction
            .accounts
            .iter()
            .take(2)
            .map(|meta| {
                (
                    meta.pubkey,
                    Account::new(1_000_000_000, 0, &Address::default()),
                )
            })
            .collect();
        mollusk.process_and_validate_instruction(&instruction, &accounts, &[Check::err(expected)]);
    }
}

#[test]
fn create_program_accepts_the_sdk_viewing_key() {
    let terms = sample_terms();
    let inputs = create_inputs(&terms);
    let (proof, spp) = proof_and_spp(CircuitId::CreateDvp, &inputs);
    assert_program_boundary(
        *inputs.public.dvp_owner.viewing_pubkey.as_bytes(),
        |viewing_pubkey| {
            CreateDvp {
                payer: Address::new_from_array([99; 32]),
                creator: terms.creator,
                tree: Address::default(),
                dvp_id: terms.dvp_id().unwrap(),
                bump: terms.dvp_authority().unwrap().1,
                viewing_pubkey,
                create_proof: proof,
                spp: spp.clone(),
            }
            .instruction()
            .unwrap()
        },
    );
}

#[test]
fn reclaim_program_accepts_the_sdk_viewing_key() {
    let terms = sample_terms();
    let inputs = reclaim_inputs(false);
    let (proof, spp) = proof_and_spp(CircuitId::ReclaimDvp, &inputs);
    assert_program_boundary(
        *inputs.public.dvp_owner.viewing_pubkey.as_bytes(),
        |viewing_pubkey| {
            ReclaimDvp {
                payer: Address::new_from_array([99; 32]),
                signer: terms.seller,
                tree: Address::default(),
                creator: terms.creator,
                dvp_id: terms.dvp_id().unwrap(),
                bump: terms.dvp_authority().unwrap().1,
                viewing_pubkey,
                reclaim_proof: proof,
                spp: spp.clone(),
            }
            .instruction()
            .unwrap()
        },
    );
}

#[test]
fn settle_program_accepts_the_sdk_viewing_key() {
    let terms = sample_terms();
    let inputs = settle_inputs([17, 23]);
    let (proof, spp) = proof_and_spp(CircuitId::SettleDvp, &inputs);
    assert_program_boundary(
        *inputs.public.dvp_owner.viewing_pubkey.as_bytes(),
        |viewing_pubkey| {
            SettleDvp {
                payer: Address::new_from_array([99; 32]),
                settlement_authority: terms.settlement_authority,
                tree: Address::default(),
                creator: terms.creator,
                dvp_id: terms.dvp_id().unwrap(),
                bump: terms.dvp_authority().unwrap().1,
                viewing_pubkey,
                current_timestamp: NOW as i64,
                settle_proof: proof,
                spp: spp.clone(),
            }
            .instruction()
            .unwrap()
        },
    );
}

#[test]
fn cancel_program_accepts_the_sdk_viewing_key() {
    let terms = sample_terms();
    let inputs = abort_inputs(false, false, true, true);
    let (proof, spp) = proof_and_spp(CircuitId::AbortDvp, &inputs);
    assert_program_boundary(
        *inputs.public.dvp_owner.viewing_pubkey.as_bytes(),
        |viewing_pubkey| {
            CancelDvp {
                payer: Address::new_from_array([99; 32]),
                settlement_authority: terms.settlement_authority,
                tree: Address::default(),
                creator: terms.creator,
                dvp_id: terms.dvp_id().unwrap(),
                bump: terms.dvp_authority().unwrap().1,
                viewing_pubkey,
                cancel_proof: proof,
                spp: spp.clone(),
            }
            .instruction()
            .unwrap()
        },
    );
}

#[test]
fn reject_program_accepts_the_sdk_viewing_key() {
    let terms = sample_terms();
    let inputs = abort_inputs(true, false, true, true);
    let (proof, spp) = proof_and_spp(CircuitId::AbortDvp, &inputs);
    assert_program_boundary(
        *inputs.public.dvp_owner.viewing_pubkey.as_bytes(),
        |viewing_pubkey| {
            RejectDvp {
                payer: Address::new_from_array([99; 32]),
                signer: terms.seller,
                tree: Address::default(),
                creator: terms.creator,
                dvp_id: terms.dvp_id().unwrap(),
                bump: terms.dvp_authority().unwrap().1,
                viewing_pubkey,
                reject_proof: proof,
                spp: spp.clone(),
            }
            .instruction()
            .unwrap()
        },
    );
}

// The SPP account checks (`SppTransactAccounts::new`) and the CPI run after
// proof verification, so the placeholder proof these fixtures carry cannot
// reach them.

use dvp_program::{error::DvpError, tag, DVP_AUTHORITY_PDA_SEED};
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use zolana_account_checks::error::AccountError;
use zolana_test_utils::mollusk::expect_err_exact;

use crate::common::{
    create_dvp_data, create_dvp_fixture, dvp_authority, program_id, setup_mollusk, CREATOR, DVP_ID,
};

/// Runs the create_dvp fixture after `edit` and expects exactly `error`.
fn expect_create_dvp_err(edit: impl FnOnce(&mut Instruction), error: impl Into<ProgramError>) {
    let (mut instruction, accounts) = create_dvp_fixture();
    edit(&mut instruction);
    expect_err_exact(&setup_mollusk(), &instruction, &accounts, error.into());
}

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    expect_create_dvp_err(
        |instruction| instruction.data = vec![tag::CREATE_DVP, 1, 2, 3],
        DvpError::InvalidInstructionData,
    );
}

#[test]
fn trailing_instruction_data_is_rejected_exactly() {
    expect_create_dvp_err(
        |instruction| instruction.data.push(0),
        DvpError::InvalidInstructionData,
    );
}

#[test]
fn missing_accounts_are_rejected_exactly() {
    // No payer, then a payer without the creator.
    for kept in [0, 1] {
        expect_create_dvp_err(
            |instruction| instruction.accounts.truncate(kept),
            AccountError::NotEnoughAccountKeys,
        );
    }
}

#[test]
fn unsigned_creator_is_rejected_exactly() {
    expect_create_dvp_err(
        |instruction| {
            instruction
                .accounts
                .get_mut(1)
                .expect("creator account meta")
                .is_signer = false;
        },
        AccountError::InvalidSigner,
    );
}

#[test]
fn bump_without_a_pda_is_rejected_exactly() {
    // A bump whose seeds hash onto the curve has no PDA.
    let bump = (0..=u8::MAX)
        .find(|bump| {
            Pubkey::create_program_address(
                &[DVP_AUTHORITY_PDA_SEED, CREATOR.as_ref(), &DVP_ID, &[*bump]],
                &program_id(),
            )
            .is_err()
        })
        .expect("some bump lands on the curve");
    expect_create_dvp_err(
        |instruction| instruction.data = create_dvp_data(DVP_ID, bump),
        DvpError::InvalidDvpAuthority,
    );
}

#[test]
fn over_modulus_dvp_id_fails_hashing_exactly() {
    // PDA seeds take any bytes, so the PDA and its owner hash still derive;
    // the over-modulus `dvp_id` fails in the public input hash.
    let dvp_id = [0xff; 32];
    let (_, bump) = dvp_authority(&CREATOR, &dvp_id);
    expect_create_dvp_err(
        |instruction| instruction.data = create_dvp_data(dvp_id, bump),
        DvpError::HashingFailed,
    );
}

#[test]
fn invalid_create_proof_is_rejected_exactly() {
    expect_create_dvp_err(|_| {}, DvpError::ProofVerificationFailed);
}

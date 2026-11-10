use mollusk_svm::Mollusk;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use spr_account_checks::error::AccountError;
use spr_test_utils::mollusk::expect_err_exact;
use timelock_escrow_program::{error::TimelockEscrowError, tag};

use crate::common::{
    edit_payer, invalid_bump, setup_mollusk, setup_unlocked_mollusk, withdraw_data,
    withdraw_data_with_bump, withdraw_fixture, UNLOCK_TIMESTAMP,
};

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
        |instruction| instruction.data = vec![tag::WITHDRAW, 1, 2, 3],
        TimelockEscrowError::InvalidInstructionData,
    );
}

#[test]
fn trailing_instruction_data_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
        |instruction| instruction.data.push(0),
        TimelockEscrowError::InvalidInstructionData,
    );
}

#[test]
fn truncated_transact_data_is_rejected_exactly() {
    // The proof and unlock timestamp parse, but the transact data after them
    // lacks its last byte.
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
        |instruction| {
            instruction.data.pop();
        },
        TimelockEscrowError::InvalidInstructionData,
    );
}

#[test]
fn missing_accounts_are_rejected_exactly() {
    // No caller, then a caller without the creator.
    for kept in [0, 1] {
        expect_withdraw_err(
            &setup_unlocked_mollusk(),
            |instruction| instruction.accounts.truncate(kept),
            AccountError::NotEnoughAccountKeys,
        );
    }
}

#[test]
fn unsigned_caller_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
        |instruction| edit_payer(instruction, |meta| meta.is_signer = false),
        AccountError::InvalidSigner,
    );
}

#[test]
fn unsigned_creator_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
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
fn readonly_caller_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
        |instruction| edit_payer(instruction, |meta| meta.is_writable = false),
        AccountError::AccountNotMutable,
    );
}

/// `now > unlock` is strict: a withdraw at the unlock second, or before it,
/// fails before the proof is checked.
#[test]
fn withdraw_before_unlock_is_rejected_exactly() {
    for now in [0, UNLOCK_TIMESTAMP] {
        let mut mollusk = setup_mollusk();
        mollusk.sysvars.clock.unix_timestamp = i64::try_from(now).expect("fits i64");
        expect_withdraw_err(&mollusk, |_| {}, TimelockEscrowError::NotYetUnlocked);
    }
}

#[test]
fn negative_clock_is_rejected_exactly() {
    let mut mollusk = setup_mollusk();
    mollusk.sysvars.clock.unix_timestamp = -1;
    expect_withdraw_err(
        &mollusk,
        |instruction| instruction.data = withdraw_data(0),
        TimelockEscrowError::NotYetUnlocked,
    );
}

#[test]
fn invalid_withdraw_proof_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_unlocked_mollusk(),
        |_| {},
        TimelockEscrowError::ProofVerificationFailed,
    );
}

/// A bump that derives no program address fails before the unlock and
/// proof checks: the clock here is before the unlock.
#[test]
fn invalid_authority_bump_is_rejected_exactly() {
    expect_withdraw_err(
        &setup_mollusk(),
        |instruction| instruction.data = withdraw_data_with_bump(UNLOCK_TIMESTAMP, invalid_bump()),
        TimelockEscrowError::InvalidEscrowAuthorityBump,
    );
}

/// Runs the withdraw fixture after `edit` and expects exactly `error`.
fn expect_withdraw_err(
    mollusk: &Mollusk,
    edit: impl FnOnce(&mut Instruction),
    error: impl Into<ProgramError>,
) {
    let (mut instruction, accounts) = withdraw_fixture();
    edit(&mut instruction);
    expect_err_exact(mollusk, &instruction, &accounts, error.into());
}

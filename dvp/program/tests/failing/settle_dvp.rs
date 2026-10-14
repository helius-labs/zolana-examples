// The SPP account checks (`SppTransactAccounts::new`) and the CPI run after
// proof verification, so the placeholder proof these fixtures carry cannot
// reach them. Every test warps the clock to `SETTLE_NOW`; Mollusk's default
// clock is 0.

use dvp_program::{error::DvpError, tag, TIME_WINDOW};
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use zolana_account_checks::error::AccountError;
use zolana_test_utils::mollusk::expect_err_exact;

use crate::common::{
    dvp_authority, non_canonical_bump, on_curve_bump, settle_dvp_data, settle_dvp_fixture,
    setup_mollusk, CREATOR, DVP_ID, SETTLE_NOW,
};

/// Runs the settle_dvp fixture after `edit` at clock `now` and expects
/// exactly `error`.
fn expect_settle_dvp_err_at(
    now: i64,
    edit: impl FnOnce(&mut Instruction),
    error: impl Into<ProgramError>,
) {
    let (mut instruction, accounts) = settle_dvp_fixture();
    edit(&mut instruction);
    let mut mollusk = setup_mollusk();
    mollusk.sysvars.clock.unix_timestamp = now;
    expect_err_exact(&mollusk, &instruction, &accounts, error.into());
}

/// [`expect_settle_dvp_err_at`] at clock `SETTLE_NOW`.
fn expect_settle_dvp_err(edit: impl FnOnce(&mut Instruction), error: impl Into<ProgramError>) {
    expect_settle_dvp_err_at(SETTLE_NOW, edit, error);
}

/// Settle data for the fixture's PDA at `current_timestamp`.
fn settle_at(current_timestamp: i64) -> Vec<u8> {
    let (_, bump) = dvp_authority(&CREATOR, &DVP_ID);
    settle_dvp_data(CREATOR, DVP_ID, bump, current_timestamp)
}

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    expect_settle_dvp_err(
        |instruction| instruction.data = vec![tag::SETTLE_DVP, 1, 2, 3],
        DvpError::InvalidInstructionData,
    );
}

#[test]
fn trailing_instruction_data_is_rejected_exactly() {
    expect_settle_dvp_err(
        |instruction| instruction.data.push(0),
        DvpError::InvalidInstructionData,
    );
}

#[test]
fn missing_accounts_are_rejected_exactly() {
    // No payer, then a payer without the settlement authority.
    for kept in [0, 1] {
        expect_settle_dvp_err(
            |instruction| instruction.accounts.truncate(kept),
            AccountError::NotEnoughAccountKeys,
        );
    }
}

#[test]
fn unsigned_settlement_authority_is_rejected_exactly() {
    expect_settle_dvp_err(
        |instruction| {
            instruction
                .accounts
                .get_mut(1)
                .expect("settlement authority account meta")
                .is_signer = false;
        },
        AccountError::InvalidSigner,
    );
}

#[test]
fn future_timestamp_is_rejected_exactly() {
    expect_settle_dvp_err(
        |instruction| instruction.data = settle_at(SETTLE_NOW + 1),
        DvpError::TimestampOutOfWindow,
    );
}

#[test]
fn timestamp_older_than_the_window_is_rejected_exactly() {
    expect_settle_dvp_err(
        |instruction| instruction.data = settle_at(SETTLE_NOW - TIME_WINDOW - 1),
        DvpError::TimestampOutOfWindow,
    );
}

#[test]
fn negative_timestamp_is_rejected_exactly() {
    // D5: negative even where the window reaches below zero. Two checks
    // refuse it with this error, the window check and the public input's
    // u64 encoding; windows.rs pins the window check alone.
    expect_settle_dvp_err_at(
        TIME_WINDOW / 2,
        |instruction| instruction.data = settle_at(-1),
        DvpError::TimestampOutOfWindow,
    );
}

/// A bump other than the canonical one: on the curve it derives no PDA;
/// below the canonical one it derives a PDA of its own, which owns no DvP.
#[test]
fn non_canonical_bump_is_rejected_exactly() {
    for bump in [on_curve_bump(), non_canonical_bump()] {
        expect_settle_dvp_err(
            |instruction| instruction.data = settle_dvp_data(CREATOR, DVP_ID, bump, SETTLE_NOW),
            DvpError::InvalidDvpAuthority,
        );
    }
}

#[test]
fn over_modulus_dvp_id_fails_hashing_exactly() {
    // PDA seeds take any bytes, so the PDA and its owner hash still derive;
    // the over-modulus `dvp_id` fails in the public input hash.
    let dvp_id = [0xff; 32];
    let (_, bump) = dvp_authority(&CREATOR, &dvp_id);
    expect_settle_dvp_err(
        |instruction| instruction.data = settle_dvp_data(CREATOR, dvp_id, bump, SETTLE_NOW),
        DvpError::HashingFailed,
    );
}

#[test]
fn invalid_settle_proof_is_rejected_exactly() {
    expect_settle_dvp_err(|_| {}, DvpError::ProofVerificationFailed);
}

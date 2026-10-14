// The SPP account checks (`SppTransactAccounts::new`) and the CPI run after
// proof verification, so the placeholder proof these fixtures carry cannot
// reach them. The abort proof's `mode` is set from the tag, so a cancel proof
// fails as a reject; a placeholder proof cannot show that difference, which
// the abort_dvp circuit tests and the cancel e2e (its proof re-tagged as a
// reject) cover.

use dvp_program::{error::DvpError, tag};
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use zolana_account_checks::error::AccountError;
use zolana_test_utils::mollusk::expect_err_exact;

use crate::common::{
    dvp_authority, non_canonical_bump, on_curve_bump, reject_dvp_data, reject_dvp_fixture,
    setup_mollusk, CREATOR, DVP_ID,
};

/// Runs the reject_dvp fixture after `edit` and expects exactly `error`.
fn expect_reject_dvp_err(edit: impl FnOnce(&mut Instruction), error: impl Into<ProgramError>) {
    let (mut instruction, accounts) = reject_dvp_fixture();
    edit(&mut instruction);
    expect_err_exact(&setup_mollusk(), &instruction, &accounts, error.into());
}

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    expect_reject_dvp_err(
        |instruction| instruction.data = vec![tag::REJECT_DVP, 1, 2, 3],
        DvpError::InvalidInstructionData,
    );
}

#[test]
fn trailing_instruction_data_is_rejected_exactly() {
    expect_reject_dvp_err(
        |instruction| instruction.data.push(0),
        DvpError::InvalidInstructionData,
    );
}

#[test]
fn missing_accounts_are_rejected_exactly() {
    // No payer, then a payer without the signer.
    for kept in [0, 1] {
        expect_reject_dvp_err(
            |instruction| instruction.accounts.truncate(kept),
            AccountError::NotEnoughAccountKeys,
        );
    }
}

#[test]
fn unsigned_signer_is_rejected_exactly() {
    expect_reject_dvp_err(
        |instruction| {
            instruction
                .accounts
                .get_mut(1)
                .expect("signer account meta")
                .is_signer = false;
        },
        AccountError::InvalidSigner,
    );
}

/// A bump other than the canonical one: on the curve it derives no PDA;
/// below the canonical one it derives a PDA of its own, which owns no DvP.
#[test]
fn non_canonical_bump_is_rejected_exactly() {
    for bump in [on_curve_bump(), non_canonical_bump()] {
        expect_reject_dvp_err(
            |instruction| instruction.data = reject_dvp_data(CREATOR, DVP_ID, bump),
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
    expect_reject_dvp_err(
        |instruction| instruction.data = reject_dvp_data(CREATOR, dvp_id, bump),
        DvpError::HashingFailed,
    );
}

#[test]
fn invalid_reject_proof_is_rejected_exactly() {
    expect_reject_dvp_err(|_| {}, DvpError::ProofVerificationFailed);
}

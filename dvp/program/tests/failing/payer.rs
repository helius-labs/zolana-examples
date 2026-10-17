use solana_instruction::AccountMeta;
use zolana_account_checks::error::AccountError;
use zolana_test_utils::mollusk::expect_err_exact;

use crate::common::{
    cancel_dvp_fixture, create_dvp_fixture, edit_payer, reclaim_dvp_fixture, reject_dvp_fixture,
    settle_dvp_fixture, setup_mollusk,
};

#[test]
fn unsigned_payer_is_rejected_exactly() {
    expect_payer_err(|meta| meta.is_signer = false, AccountError::InvalidSigner);
}

#[test]
fn readonly_payer_is_rejected_exactly() {
    expect_payer_err(
        |meta| meta.is_writable = false,
        AccountError::AccountNotMutable,
    );
}

/// Runs every instruction's fixture with `edit` applied to the payer and
/// expects exactly `error`.
fn expect_payer_err(edit: impl Fn(&mut AccountMeta), error: AccountError) {
    let mollusk = setup_mollusk();
    for (mut instruction, accounts) in [
        create_dvp_fixture(),
        reclaim_dvp_fixture(),
        settle_dvp_fixture(),
        cancel_dvp_fixture(),
        reject_dvp_fixture(),
    ] {
        edit_payer(&mut instruction, &edit);
        expect_err_exact(&mollusk, &instruction, &accounts, error.into());
    }
}

use light_program_profiler::profile;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use spr_program::cpi::{SppTransactAccounts, TransactAccountsError};

use crate::error::TimelockEscrowError;

#[inline(always)]
pub fn check_after_window(now: i64, unlock_unix_ts: u64) -> ProgramResult {
    if now >= 0 && (now as u64) > unlock_unix_ts {
        Ok(())
    } else {
        Err(TimelockEscrowError::NotYetUnlocked.into())
    }
}

/// Invokes SPP `transact` with `spp_accounts`, signed by the escrow-authority
/// PDA, which must be among them.
#[inline(never)]
#[profile]
pub fn cpi_spp_transact_signed(
    spp_accounts: &[AccountView],
    transact_bytes: &[u8],
) -> ProgramResult {
    let (escrow_authority, bump) =
        Address::find_program_address(&[crate::ESCROW_AUTHORITY_PDA_SEED], &crate::ID);
    let signer_pdas = [&escrow_authority];
    let spp =
        SppTransactAccounts::new(spp_accounts, &signer_pdas).map_err(transact_accounts_error)?;
    let bump = [bump];
    let seeds = [
        Seed::from(crate::ESCROW_AUTHORITY_PDA_SEED),
        Seed::from(&bump),
    ];
    spp.invoke::<16>(transact_bytes, &[Signer::from(&seeds)])
}

fn transact_accounts_error(error: TransactAccountsError) -> ProgramError {
    match error {
        TransactAccountsError::InvalidSppProgram => {
            TimelockEscrowError::InvalidShieldedPoolProgram.into()
        }
        TransactAccountsError::MissingPdaSigner { .. } => {
            TimelockEscrowError::MissingEscrowAuthority.into()
        }
        TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
    }
}

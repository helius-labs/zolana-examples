use light_program_profiler::profile;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use spr_interface::instruction::instruction_data::transact::TransactIxDataRef;
use spr_program::cpi::{SppTransactAccounts, TransactAccountsError};
use wincode::{config::DefaultConfig, SchemaReadOwned};

use crate::error::SwapError;

/// Instruction data: the instruction's own `args`, then the SPP `transact`
/// data. The program reads the transact fields it checks in place and
/// forwards `transact_bytes` to SPP unchanged.
pub struct IxData<'a, A> {
    pub args: A,
    pub transact: TransactIxDataRef<'a>,
    pub transact_bytes: &'a [u8],
}

impl<'a, A: SchemaReadOwned<DefaultConfig, Dst = A>> IxData<'a, A> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ProgramError> {
        let mut transact_bytes = data;
        let args = wincode::deserialize_from(&mut transact_bytes)
            .map_err(|_| SwapError::InvalidInstructionData)?;
        let transact = TransactIxDataRef::from_bytes(transact_bytes)
            .map_err(|_| SwapError::InvalidInstructionData)?;
        Ok(Self {
            args,
            transact,
            transact_bytes,
        })
    }
}

#[inline(always)]
pub fn check_within_window(now: i64, expiry_unix_ts: u64) -> ProgramResult {
    if now >= 0 && (now as u64) <= expiry_unix_ts {
        Ok(())
    } else {
        Err(SwapError::Expired.into())
    }
}

#[inline(always)]
pub fn check_after_window(now: i64, expiry_unix_ts: u64) -> ProgramResult {
    if now >= 0 && (now as u64) > expiry_unix_ts {
        Ok(())
    } else {
        Err(SwapError::NotYetExpired.into())
    }
}

/// Invokes SPP `transact` with `spp_accounts`, signed by the order-authority
/// PDA, which must be among them.
#[inline(never)]
#[profile]
pub fn cpi_spp_transact_signed(
    spp_accounts: &[AccountView],
    transact_bytes: &[u8],
) -> ProgramResult {
    let (order_authority, bump) =
        Address::find_program_address(&[crate::ORDER_AUTHORITY_PDA_SEED], &crate::ID);
    let signer_pdas = [&order_authority];
    let spp =
        SppTransactAccounts::new(spp_accounts, &signer_pdas).map_err(transact_accounts_error)?;
    let bump = [bump];
    let seeds = [
        Seed::from(crate::ORDER_AUTHORITY_PDA_SEED),
        Seed::from(&bump),
    ];
    spp.invoke::<16>(transact_bytes, &[Signer::from(&seeds)])
}

fn transact_accounts_error(error: TransactAccountsError) -> ProgramError {
    match error {
        TransactAccountsError::InvalidSppProgram => SwapError::InvalidShieldedPoolProgram.into(),
        TransactAccountsError::MissingPdaSigner { .. } => SwapError::MissingOrderAuthority.into(),
        TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
    }
}

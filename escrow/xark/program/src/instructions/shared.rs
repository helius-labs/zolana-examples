use light_program_profiler::profile;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use spr_interface::instruction::instruction_data::transact::TransactIxDataRef;
use spr_program::cpi::{SppTransactAccounts, TransactAccountsError};
use wincode::{config::DefaultConfig, SchemaReadOwned};

use crate::error::TimelockEscrowError;

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
            .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;
        let transact = TransactIxDataRef::from_bytes(transact_bytes)
            .map_err(|_| TimelockEscrowError::InvalidInstructionData)?;
        Ok(Self {
            args,
            transact,
            transact_bytes,
        })
    }
}

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

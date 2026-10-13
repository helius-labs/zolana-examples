use pinocchio::{error::ProgramError, AccountView, Address, ProgramResult};
use wincode::SchemaRead;
use zolana_account_checks::AccountIterator;

use crate::{error::StakingError, state::ShieldedAddress};

pub(crate) fn parse<'a, T: SchemaRead<'a, wincode::config::DefaultConfig, Dst = T>>(
    data: &'a [u8],
) -> Result<T, ProgramError> {
    wincode::deserialize_exact(data).map_err(|_| StakingError::InvalidInstructionData.into())
}

pub(crate) fn check_key(account: &AccountView, expected: &Address) -> ProgramResult {
    if account.address() != expected {
        return Err(StakingError::InvalidAccount.into());
    }
    Ok(())
}

pub(crate) fn check_system(account: &AccountView) -> ProgramResult {
    check_key(account, &pinocchio_system::ID)
}

pub(crate) fn check_crank(account: &AccountView, crank: &ShieldedAddress) -> ProgramResult {
    if !account.is_signer() || account.address() != &crank.signing_pubkey {
        return Err(StakingError::InvalidCrank.into());
    }
    Ok(())
}

pub(crate) fn no_more(iter: &AccountIterator<'_>) -> ProgramResult {
    if !iter.iterator_is_empty() {
        return Err(ProgramError::InvalidArgument);
    }
    Ok(())
}

pub(crate) fn close_to(account: &mut AccountView, recipient: &mut AccountView) -> ProgramResult {
    if account.address() == recipient.address() || !recipient.is_writable() {
        return Err(StakingError::InvalidAccount.into());
    }
    let lamports = recipient
        .lamports()
        .checked_add(account.lamports())
        .ok_or(StakingError::Overflow)?;
    recipient.set_lamports(lamports);
    account.set_lamports(0);
    account.close()
}

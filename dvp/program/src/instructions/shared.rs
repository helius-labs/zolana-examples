#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use light_program_profiler::profile;
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use pinocchio::cpi::{Seed, Signer};
use pinocchio::{error::ProgramError, AccountView, Address, ProgramResult};
use zolana_program::compression::PdaOwner;
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use zolana_program::cpi::{SppTransactAccounts, TransactAccountsError};

use crate::{error::DvpError, DVP_AUTHORITY_PDA_SEED, TIME_WINDOW};

pub fn u64_right_align(value: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let (_, low) = bytes.split_at_mut(24);
    low.copy_from_slice(&value.to_be_bytes());
    bytes
}

/// Accepts a settlement `current_timestamp` that is non-negative (D5), not
/// in the future, and at most `TIME_WINDOW` seconds behind the clock `now`.
#[inline(always)]
pub fn check_settle_timestamp(now: i64, current_timestamp: i64) -> ProgramResult {
    if current_timestamp >= 0
        && now.saturating_sub(TIME_WINDOW) <= current_timestamp
        && current_timestamp <= now
    {
        Ok(())
    } else {
        Err(DvpError::TimestampOutOfWindow.into())
    }
}

/// The per-DvP authority PDA `[b"dvp", creator, dvp_id, [bump]]`. The bump
/// must be canonical: a non-canonical bump gives another PDA, owner hash and
/// address slot, so the same `(creator, dvp_id)` could be created twice
/// (property 3).
pub fn dvp_authority(creator: &Address, dvp_id: &[u8; 32], bump: u8) -> Result<Address, DvpError> {
    match Address::try_find_program_address(
        &[DVP_AUTHORITY_PDA_SEED, creator.as_ref(), dvp_id],
        &crate::ID,
    ) {
        Some((address, canonical_bump)) if canonical_bump == bump => Ok(address),
        _ => Err(DvpError::InvalidDvpAuthority),
    }
}

/// `Poseidon(id(dvp_authority), ZERO_NULLIFIER_PUBKEY)`: the owner of the DvP
/// address slot, the DvP UTXO and the deposits.
pub fn dvp_authority_owner_hash(dvp_authority: &Address) -> Result<[u8; 32], ProgramError> {
    let owner = PdaOwner::new(dvp_authority).map_err(|_| DvpError::HashingFailed)?;
    Ok(*owner.owner_hash())
}

/// CPIs SPP transact through `spp_accounts`, signing for `dvp_authority`
/// with the seeds `[b"dvp", creator, dvp_id, [bump]]`. The PDA must be in
/// the forwarded account list (D8).
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
#[inline(never)]
#[profile]
pub fn cpi_spp_transact_signed(
    spp_accounts: &[AccountView],
    transact_bytes: &[u8],
    dvp_authority: &Address,
    creator: &Address,
    dvp_id: &[u8; 32],
    bump: u8,
) -> ProgramResult {
    let signer_pdas = [dvp_authority];
    let spp =
        SppTransactAccounts::new(spp_accounts, &signer_pdas).map_err(transact_accounts_error)?;
    let bump = [bump];
    let seeds = [
        Seed::from(DVP_AUTHORITY_PDA_SEED),
        Seed::from(creator.as_ref()),
        Seed::from(dvp_id.as_slice()),
        Seed::from(&bump),
    ];
    spp.invoke::<16>(transact_bytes, &[Signer::from(&seeds)])
}

#[cfg(any(target_os = "solana", target_arch = "bpf"))]
fn transact_accounts_error(error: TransactAccountsError) -> ProgramError {
    match error {
        TransactAccountsError::InvalidSppProgram => DvpError::InvalidShieldedPoolProgram.into(),
        TransactAccountsError::MissingPdaSigner { .. } => DvpError::MissingDvpAuthority.into(),
        TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
    }
}

#[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
#[inline(never)]
pub fn cpi_spp_transact_signed(
    _spp_accounts: &[AccountView],
    _transact_bytes: &[u8],
    _dvp_authority: &Address,
    _creator: &Address,
    _dvp_id: &[u8; 32],
    _bump: u8,
) -> ProgramResult {
    unimplemented!("cpi_spp_transact_signed requires Solana runtime syscalls")
}

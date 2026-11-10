#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use light_program_profiler::profile;
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio::{AccountView, Address, ProgramResult};
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use spr_program::cpi::{SppTransactAccounts, TransactAccountsError};
use spr_program::program::PdaOwner;

use crate::error::TimelockEscrowError;

pub fn u64_right_align(value: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[24..32].copy_from_slice(&value.to_be_bytes());
    bytes
}

#[inline(always)]
pub fn check_after_window(now: i64, unlock_unix_ts: u64) -> ProgramResult {
    if now >= 0 && (now as u64) > unlock_unix_ts {
        Ok(())
    } else {
        Err(TimelockEscrowError::NotYetUnlocked.into())
    }
}

/// The creator's escrow-authority PDA (seeds `[ESCROW_AUTHORITY_PDA_SEED,
/// creator]`). It owns the creator's escrow utxos with nullifier secret 0,
/// and only this program signs for it.
pub struct EscrowAuthority {
    address: Address,
    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    creator: Address,
    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    bump: u8,
}

impl EscrowAuthority {
    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    pub fn find(creator: &Address) -> Self {
        let (address, bump) =
            Address::find_program_address(&crate::escrow_authority_seeds(creator), &crate::ID);
        Self {
            address,
            creator: *creator,
            bump,
        }
    }

    #[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
    pub fn find(_creator: &Address) -> Self {
        unimplemented!("EscrowAuthority::find requires Solana runtime syscalls")
    }

    /// `Poseidon(owner_identity(pda), nullifier_pubkey(0))`, the owner hash of
    /// every escrow utxo this PDA owns.
    pub fn owner_hash(&self) -> Result<[u8; 32], TimelockEscrowError> {
        Ok(*PdaOwner::new(&self.address)
            .map_err(|_| TimelockEscrowError::HashingFailed)?
            .owner_hash())
    }

    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    #[inline(never)]
    #[profile]
    pub fn cpi_spp_transact_signed(
        &self,
        spp_accounts: &[AccountView],
        transact: &[u8],
    ) -> ProgramResult {
        let signer_pdas = [&self.address];
        let spp = SppTransactAccounts::new(spp_accounts, &signer_pdas)
            .map_err(transact_accounts_error)?;
        let bump = [self.bump];
        let seeds = [
            Seed::from(crate::ESCROW_AUTHORITY_PDA_SEED),
            Seed::from(self.creator.as_array()),
            Seed::from(&bump),
        ];
        spp.invoke::<16>(transact, &[Signer::from(&seeds)])
    }

    #[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
    pub fn cpi_spp_transact_signed(
        &self,
        _spp_accounts: &[AccountView],
        _transact: &[u8],
    ) -> ProgramResult {
        unimplemented!("EscrowAuthority::cpi_spp_transact_signed requires Solana runtime syscalls")
    }
}

#[cfg(any(target_os = "solana", target_arch = "bpf"))]
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

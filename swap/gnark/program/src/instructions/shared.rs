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

use crate::error::SwapError;

pub fn u64_right_align(value: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[24..32].copy_from_slice(&value.to_be_bytes());
    bytes
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

/// The maker's order-authority PDA, seeds `[ORDER_AUTHORITY_PDA_SEED, maker]`.
/// It owns every order UTXO the maker creates (nullifier secret 0). The
/// program signs for it to create them in make, where the maker signs for its
/// own source, and to spend them in take and cancel.
pub struct OrderAuthority {
    address: Address,
    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    maker: Address,
    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    bump: u8,
}

impl OrderAuthority {
    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    pub fn find(maker: &Address) -> Self {
        let (address, bump) =
            Address::find_program_address(&crate::order_authority_seeds(maker), &crate::ID);
        Self {
            address,
            maker: *maker,
            bump,
        }
    }

    #[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
    pub fn find(_maker: &Address) -> Self {
        unimplemented!("OrderAuthority::find requires Solana runtime syscalls")
    }

    /// `Poseidon(solana_owner_identity(pda), Poseidon(0))`: the owner hash the
    /// make circuit binds the order UTXO to.
    pub fn owner_hash(&self) -> Result<[u8; 32], SwapError> {
        Ok(*PdaOwner::new(&self.address)
            .map_err(|_| SwapError::HashingFailed)?
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
            Seed::from(crate::ORDER_AUTHORITY_PDA_SEED),
            Seed::from(self.maker.as_array()),
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
        unimplemented!("OrderAuthority::cpi_spp_transact_signed requires Solana runtime syscalls")
    }
}

#[cfg(any(target_os = "solana", target_arch = "bpf"))]
fn transact_accounts_error(error: TransactAccountsError) -> ProgramError {
    match error {
        TransactAccountsError::InvalidSppProgram => SwapError::InvalidShieldedPoolProgram.into(),
        TransactAccountsError::MissingPdaSigner { .. } => SwapError::MissingOrderAuthority.into(),
        TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
    }
}

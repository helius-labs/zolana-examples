use light_program_profiler::profile;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use spr_interface::instruction::instruction_data::transact::TransactIxDataRef;
use spr_program::{
    cpi::{SppTransactAccounts, TransactAccountsError},
    program::PdaOwner,
};
use wincode::{config::DefaultConfig, SchemaReadOwned};

use crate::error::SwapError;

pub fn u64_right_align(value: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[24..32].copy_from_slice(&value.to_be_bytes());
    bytes
}

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

/// The maker's order-authority PDA, seeds `[ORDER_AUTHORITY_PDA_SEED, maker,
/// bump]`. It owns every order UTXO the maker creates (nullifier secret 0).
/// The program signs for it to create them in make, where the maker signs for
/// its own source, and to spend them in take and cancel.
pub struct OrderAuthority {
    address: Address,
    maker: Address,
    bump: u8,
}

impl OrderAuthority {
    /// The PDA `[ORDER_AUTHORITY_PDA_SEED, maker, [bump]]`, with the bump
    /// the client passes in the instruction data instead of a
    /// `find_program_address` search, so the derivation costs one
    /// `create_program_address` for every maker. A bump that derives no
    /// program address fails with `InvalidOrderAuthorityBump`.
    ///
    /// The SDK passes the canonical bump. A non-canonical bump that still
    /// derives a program address names another PDA of the same maker: make
    /// then outputs the order UTXO to that PDA, which only that bump spends,
    /// and take and cancel sign for a PDA that owns none of the maker's
    /// canonical orders. Either way it moves only the maker's own UTXO; it
    /// signs for no other maker's PDA.
    pub fn new(maker: &Address, bump: u8) -> Result<Self, SwapError> {
        let [seed, maker_seed] = crate::order_authority_seeds(maker);
        let address = Address::create_program_address(&[seed, maker_seed, &[bump]], &crate::ID)
            .map_err(|_| SwapError::InvalidOrderAuthorityBump)?;
        Ok(Self {
            address,
            maker: *maker,
            bump,
        })
    }

    /// `Poseidon(solana_owner_identity(pda), Poseidon(0))`: the owner hash the
    /// make circuit binds the order UTXO to.
    pub fn owner_hash(&self) -> Result<[u8; 32], SwapError> {
        Ok(*PdaOwner::new(&self.address)
            .map_err(|_| SwapError::HashingFailed)?
            .owner_hash())
    }

    /// Invokes SPP `transact` with `spp_accounts`, signed by this PDA, which
    /// must be among them.
    #[inline(never)]
    #[profile]
    pub fn cpi_spp_transact_signed(
        &self,
        spp_accounts: &[AccountView],
        transact_bytes: &[u8],
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
        spp.invoke::<16>(transact_bytes, &[Signer::from(&seeds)])
    }
}

fn transact_accounts_error(error: TransactAccountsError) -> ProgramError {
    match error {
        TransactAccountsError::InvalidSppProgram => SwapError::InvalidShieldedPoolProgram.into(),
        TransactAccountsError::MissingPdaSigner { .. } => SwapError::MissingOrderAuthority.into(),
        TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
    }
}

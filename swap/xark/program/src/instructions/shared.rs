use light_program_profiler::profile;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use spr_hasher::primitives::solana_owner_identity;
use spr_interface::instruction::instruction_data::transact::TransactIxDataRef;
use spr_program::{
    cpi::{SppTransactAccounts, TransactAccountsError},
    program::PdaOwner,
    verifier::address_hash,
};
use wincode::{config::DefaultConfig, SchemaReadOwned};

use crate::{error::SwapError, verifier::make};

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

/// A maker's order-authority PDA, `[ORDER_AUTHORITY_PDA_SEED, maker]`: the
/// owner of the maker's order UTXOs under nullifier secret 0. Only the program
/// signs for it, so only take and cancel spend an order UTXO.
pub struct OrderAuthority {
    address: Address,
    maker: [u8; 32],
    bump: u8,
}

impl OrderAuthority {
    pub fn find(maker: &[u8; 32]) -> Self {
        let (address, bump) =
            Address::find_program_address(&crate::order_authority_seeds(maker), &crate::ID);
        Self {
            address,
            maker: *maker,
            bump,
        }
    }

    /// `Poseidon(solana_owner_identity(pda), Poseidon(0))`.
    fn owner_hash(&self) -> Result<[u8; 32], SwapError> {
        Ok(*PdaOwner::new(&self.address)
            .map_err(|_| SwapError::HashingFailed)?
            .owner_hash())
    }

    /// The make proof's public inputs: the maker's owner identity, and this
    /// PDA's address with the order's `viewing_pubkey` as the order UTXO's
    /// owner.
    pub fn make_public_inputs(
        &self,
        viewing_pubkey: &[u8; 33],
    ) -> Result<make::PublicInputs, SwapError> {
        Ok(make::PublicInputs {
            maker_owner_pk_field: solana_owner_identity(&self.maker)?,
            order_owner: address_hash(&self.owner_hash()?, viewing_pubkey)?,
        })
    }

    /// Invokes SPP `transact` with `spp_accounts`, signed by this PDA, which
    /// must be among them.
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
            Seed::from(&self.maker),
            Seed::from(&bump),
        ];
        spp.invoke::<16>(transact, &[Signer::from(&seeds)])
    }
}

fn transact_accounts_error(error: TransactAccountsError) -> ProgramError {
    match error {
        TransactAccountsError::InvalidSppProgram => SwapError::InvalidShieldedPoolProgram.into(),
        TransactAccountsError::MissingPdaSigner { .. } => SwapError::MissingOrderAuthority.into(),
        TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
    }
}

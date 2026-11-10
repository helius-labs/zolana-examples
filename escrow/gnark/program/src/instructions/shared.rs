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

use crate::error::TimelockEscrowError;

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

/// The creator's escrow-authority PDA (seeds `[ESCROW_AUTHORITY_PDA_SEED,
/// creator, bump]`). It owns the creator's escrow utxos with nullifier secret
/// 0, and only this program signs for it.
pub struct EscrowAuthority {
    address: Address,
    creator: Address,
    bump: u8,
}

impl EscrowAuthority {
    /// The PDA `[ESCROW_AUTHORITY_PDA_SEED, creator, [bump]]`, with the bump
    /// the client passes in the instruction data instead of a
    /// `find_program_address` search, so the derivation costs one
    /// `create_program_address` for every creator. A bump that derives no
    /// program address fails with `InvalidEscrowAuthorityBump`.
    ///
    /// The SDK passes the canonical bump. A non-canonical bump that still
    /// derives a program address names another PDA of the same creator:
    /// escrow then outputs the escrow UTXO to that PDA, which only that bump
    /// spends, and withdraw signs for a PDA that owns none of the creator's
    /// canonical escrows. Either way it moves only the creator's own UTXO; it
    /// signs for no other creator's PDA.
    pub fn new(creator: &Address, bump: u8) -> Result<Self, TimelockEscrowError> {
        let [seed, creator_seed] = crate::escrow_authority_seeds(creator);
        let address = Address::create_program_address(&[seed, creator_seed, &[bump]], &crate::ID)
            .map_err(|_| TimelockEscrowError::InvalidEscrowAuthorityBump)?;
        Ok(Self {
            address,
            creator: *creator,
            bump,
        })
    }

    /// `Poseidon(owner_identity(pda), nullifier_pubkey(0))`, the owner hash of
    /// every escrow utxo this PDA owns.
    pub fn owner_hash(&self) -> Result<[u8; 32], TimelockEscrowError> {
        Ok(*PdaOwner::new(&self.address)
            .map_err(|_| TimelockEscrowError::HashingFailed)?
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
            Seed::from(crate::ESCROW_AUTHORITY_PDA_SEED),
            Seed::from(self.creator.as_array()),
            Seed::from(&bump),
        ];
        spp.invoke::<16>(transact_bytes, &[Signer::from(&seeds)])
    }
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

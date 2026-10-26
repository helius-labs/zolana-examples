use light_program_profiler::profile;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_program::{
    cpi::{SppTransactAccounts, TransactAccountsError},
    program::PdaOwner,
    verifier::VerifyError,
};

use crate::{error::DvpError, verifier::settle_dvp, DVP_AUTHORITY_PDA_SEED};
use pinocchio::sysvars::{clock::Clock, Sysvar};

/// 1. Check payer and signer.
/// 2. Decode the instruction.
/// 3. Check the timestamp against the Clock.
/// 4. Derive the DvP PDA.
/// 5. Verify the DvP proof.
/// 6. Serialize the SPP transaction.
/// 7. Invoke SPP with the DvP PDA signer.
#[inline(never)]
#[profile]
pub fn process_settle_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check payer and signer.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    let settlement_authority = *iter.next_signer("settlement_authority")?.address();

    // 2. Decode the instruction.
    let SettleDvpIxData {
        creator,
        dvp_id,
        bump,
        viewing_pubkey,
        current_timestamp,
        settle_proof,
        spp,
    } = wincode::deserialize_exact(data).map_err(|_| DvpError::InvalidInstructionData)?;

    // 3. Check the timestamp against the Clock.
    let now = Clock::get()?.unix_timestamp;
    if current_timestamp < 0
        || current_timestamp < now.saturating_sub(crate::TIME_WINDOW)
        || current_timestamp > now
    {
        return Err(DvpError::TimestampOutOfWindow.into());
    }
    let timestamp = u64::try_from(current_timestamp).map_err(|_| DvpError::TimestampOutOfWindow)?;

    // 4. Derive the DvP PDA.
    let (dvp_authority, canonical_bump) = Address::try_find_program_address(
        &[DVP_AUTHORITY_PDA_SEED, creator.as_ref(), &dvp_id],
        &crate::ID,
    )
    .ok_or(DvpError::InvalidDvpAuthority)?;
    if canonical_bump != bump {
        return Err(DvpError::InvalidDvpAuthority.into());
    }
    let owner = PdaOwner::new(&dvp_authority).map_err(|_| DvpError::HashingFailed)?;
    let dvp_owner = zolana_program::verifier::address_hash(owner.owner_hash(), &viewing_pubkey)
        .map_err(|_| DvpError::HashingFailed)?;
    let creator_identity = solana_owner_identity(creator.as_array()).map_err(DvpError::from)?;
    let signer_identity =
        solana_owner_identity(settlement_authority.as_array()).map_err(DvpError::from)?;

    // 5. Verify the DvP proof.
    settle_dvp::Verifier {
        proof: &settle_proof,
        public_inputs: settle_dvp::PublicInputs {
            creator_identity,
            dvp_id,
            dvp_owner,
            settlement_authority_identity: signer_identity,
            current_timestamp: timestamp,
        },
        private_tx_hash: &spp.private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(|error| match error {
        VerifyError::PublicInputs => DvpError::HashingFailed,
        VerifyError::Proof => DvpError::ProofVerificationFailed,
    })?;

    // 6. Serialize the SPP transaction.
    let transact = spp
        .serialize()
        .map_err(|_| DvpError::InvalidInstructionData)?;

    // 7. Invoke SPP with the DvP PDA signer.
    let signer_pdas = [&dvp_authority];
    let accounts =
        SppTransactAccounts::new(iter.remaining()?, &signer_pdas).map_err(|error| match error {
            TransactAccountsError::InvalidSppProgram => DvpError::InvalidShieldedPoolProgram.into(),
            TransactAccountsError::MissingPdaSigner { .. } => DvpError::MissingDvpAuthority.into(),
            TransactAccountsError::NotEnoughAccounts => ProgramError::NotEnoughAccountKeys,
        })?;
    let bump = [bump];
    let seeds = [
        Seed::from(DVP_AUTHORITY_PDA_SEED),
        Seed::from(creator.as_ref()),
        Seed::from(dvp_id.as_slice()),
        Seed::from(&bump),
    ];
    accounts.invoke::<16>(&transact, &[Signer::from(&seeds)])
}

pub use zolana_program::CompressedProof as SettleDvpProof;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SettleDvpIxData {
    pub creator: Address,
    pub dvp_id: [u8; 32],
    pub bump: u8,
    pub viewing_pubkey: [u8; 33],
    pub current_timestamp: i64,
    pub settle_proof: SettleDvpProof,
    pub spp: TransactIxData,
}

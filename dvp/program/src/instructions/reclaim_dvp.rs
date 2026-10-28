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

use crate::{error::DvpError, verifier::reclaim_dvp, DVP_AUTHORITY_PDA_SEED};

/// 1. Check payer and signer.
/// 2. Decode the instruction.
/// 3. Derive the DvP PDA.
/// 4. Verify the DvP proof.
/// 5. Serialize the SPP transaction.
/// 6. Invoke SPP with the DvP PDA signer.
#[inline(never)]
#[profile]
pub fn process_reclaim_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Check payer and signer.
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    let signer = *iter.next_signer("signer")?.address();

    // 2. Decode the instruction.
    let ReclaimDvpIxData {
        creator,
        dvp_id,
        bump,
        reclaim_proof,
        spp,
    } = wincode::deserialize_exact(data).map_err(|_| DvpError::InvalidInstructionData)?;

    // 3. Derive the DvP PDA.
    let (dvp_authority, canonical_bump) = Address::try_find_program_address(
        &[DVP_AUTHORITY_PDA_SEED, creator.as_ref(), &dvp_id],
        &crate::ID,
    )
    .ok_or(DvpError::InvalidDvpAuthority)?;
    if canonical_bump != bump {
        return Err(DvpError::InvalidDvpAuthority.into());
    }
    let owner = PdaOwner::new(&dvp_authority).map_err(|_| DvpError::HashingFailed)?;
    let dvp_owner = *owner.owner_hash();
    let creator_identity = solana_owner_identity(creator.as_array()).map_err(DvpError::from)?;
    let signer_identity = solana_owner_identity(signer.as_array()).map_err(DvpError::from)?;

    // 4. Verify the DvP proof.
    reclaim_dvp::Verifier {
        proof: &reclaim_proof,
        public_inputs: reclaim_dvp::PublicInputs {
            creator_identity,
            dvp_id,
            dvp_owner,
            signer_identity,
        },
        private_tx_hash: &spp.private_tx_hash,
        transfers: &[],
    }
    .verify()
    .map_err(|error| match error {
        VerifyError::PublicInputs => DvpError::HashingFailed,
        VerifyError::Proof => DvpError::ProofVerificationFailed,
    })?;

    // 5. Serialize the SPP transaction.
    let transact = spp
        .serialize()
        .map_err(|_| DvpError::InvalidInstructionData)?;

    // 6. Invoke SPP with the DvP PDA signer.
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

pub use zolana_program::CompressedProof as ReclaimDvpProof;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ReclaimDvpIxData {
    pub creator: Address,
    pub dvp_id: [u8; 32],
    pub bump: u8,
    pub reclaim_proof: ReclaimDvpProof,
    pub spp: TransactIxData,
}

use light_program_profiler::profile;
use pinocchio::{error::ProgramError, AccountView, Address, ProgramResult};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_hasher::{primitives::solana_owner_identity, Hasher, Poseidon};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    error::DvpError,
    instructions::{
        shared::{cpi_spp_transact_signed, dvp_authority, dvp_authority_owner_hash},
        verifier::{verify_groth16, CompressedGroth16Proof},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ReclaimDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ReclaimDvpIxData {
    /// The terms' creator. With `dvp_id` and `bump` it seeds the PDA; the
    /// reclaim proof checks it against the terms.
    pub creator: Address,
    pub dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority` (`find_program_address`).
    pub bump: u8,
    pub reclaim_proof: ReclaimDvpProof,
    pub spp: TransactIxData,
}

/// The reclaim proof's single public input. The program supplies every
/// element: `private_tx_hash` from the SPP transact it CPIs, `creator_id` and
/// `dvp_id` from the instruction data, the owner hash from the PDA it derives
/// from both, and `signer_id` from the reclaiming signer.
pub struct ReclaimDvpPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// `solana_owner_identity(creator)`.
    pub creator_id: &'a [u8; 32],
    pub dvp_id: &'a [u8; 32],
    /// `Poseidon(solana_owner_identity(dvp_authority), ZERO_NULLIFIER_PUBKEY)`.
    pub dvp_authority_owner_hash: &'a [u8; 32],
    /// `solana_owner_identity(signer)`: the seller or the buyer, whichever
    /// funded the reclaimed leg.
    pub signer_id: &'a [u8; 32],
}

impl ReclaimDvpPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.creator_id.as_slice(),
            self.dvp_id.as_slice(),
            self.dvp_authority_owner_hash.as_slice(),
            self.signer_id.as_slice(),
        ])
        .map_err(|_| DvpError::HashingFailed.into())
    }
}

/// Reclaims one deposit: the SPP transact spends a deposit owned by
/// `dvp_authority` and returns its full amount to an owner the signer
/// chooses. The reclaim proof binds that transact (`private_tx_hash`) to the
/// signing party, whose identity selects the leg (seller: delivery, buyer:
/// payment), and to the terms behind `dvp_id`.
///
/// As the spec says, there is no expiry check and no check that the DvP is
/// open: the DvP UTXO is not an input, so a deposit is reclaimable before or
/// after expiry, settlement or abort. The PDA is forwarded as the last SPP
/// account, the owner-signer run.
#[inline(never)]
#[profile]
pub fn process_reclaim_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let mut iter = AccountIterator::new(accounts);
    iter.next_signer_mut("payer")?;
    // The reclaiming party signs; its identity is a public input, so only
    // the funder of a leg's terms can take that leg's deposits.
    let signer = iter.next_signer("signer")?.address();
    let signer_id = solana_owner_identity(signer.as_array()).map_err(DvpError::from)?;

    let ReclaimDvpIxData {
        creator,
        dvp_id,
        bump,
        reclaim_proof,
        spp,
    } = wincode::deserialize_exact(data).map_err(|_| DvpError::InvalidInstructionData)?;
    let creator_id = solana_owner_identity(creator.as_array()).map_err(DvpError::from)?;

    let dvp_authority = dvp_authority(&creator, &dvp_id, bump)?;
    let dvp_authority_owner_hash = dvp_authority_owner_hash(&dvp_authority)?;

    verify_groth16(
        CompressedGroth16Proof {
            a: &reclaim_proof.proof_a,
            b: &reclaim_proof.proof_b,
            c: &reclaim_proof.proof_c,
            commitment: None,
        },
        ReclaimDvpPublicInput {
            private_tx_hash: &spp.private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &dvp_authority_owner_hash,
            signer_id: &signer_id,
        }
        .hash()?,
        &crate::verifying_keys::reclaim_dvp::VERIFYINGKEY,
    )?;

    let spp_bytes = spp
        .serialize()
        .map_err(|_| DvpError::InvalidInstructionData)?;
    let spp_accounts = iter.remaining()?;
    cpi_spp_transact_signed(
        spp_accounts,
        &spp_bytes,
        &dvp_authority,
        &creator,
        &dvp_id,
        bump,
    )
}

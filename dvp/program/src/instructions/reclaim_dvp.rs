use pinocchio::error::ProgramError;
use wincode::{SchemaRead, SchemaWrite};
use zolana_hasher::{Hasher, Poseidon};

use crate::error::DvpError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ReclaimDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
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

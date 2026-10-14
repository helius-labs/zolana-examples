use pinocchio::error::ProgramError;
use wincode::{SchemaRead, SchemaWrite};
use zolana_hasher::{Hasher, Poseidon};

use crate::{error::DvpError, instructions::shared::u64_right_align};

/// The proof of the abort_dvp circuit, which cancel_dvp and reject_dvp share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct AbortDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The abort proof's single public input. The program supplies every
/// element: `private_tx_hash` from the SPP transact it CPIs, `creator_id` and
/// `dvp_id` from the instruction data, the owner hash from the PDA it
/// derives, `signer_id` from the aborting signer and `mode` from the
/// instruction tag.
pub struct AbortDvpPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// `solana_owner_identity(creator)`.
    pub creator_id: &'a [u8; 32],
    pub dvp_id: &'a [u8; 32],
    /// `Poseidon(solana_owner_identity(dvp_authority), ZERO_NULLIFIER_PUBKEY)`.
    pub dvp_authority_owner_hash: &'a [u8; 32],
    /// `solana_owner_identity(signer)`: the settlement authority in cancel
    /// mode, the seller or the buyer in reject mode.
    pub signer_id: &'a [u8; 32],
    /// `ABORT_MODE_CANCEL` or `ABORT_MODE_REJECT`, which picks the signer
    /// rule the circuit applies.
    pub mode: u64,
}

impl AbortDvpPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.creator_id.as_slice(),
            self.dvp_id.as_slice(),
            self.dvp_authority_owner_hash.as_slice(),
            self.signer_id.as_slice(),
            u64_right_align(self.mode).as_slice(),
        ])
        .map_err(|_| DvpError::HashingFailed.into())
    }
}

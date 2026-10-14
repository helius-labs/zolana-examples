use pinocchio::error::ProgramError;
use wincode::{SchemaRead, SchemaWrite};
use zolana_hasher::{Hasher, Poseidon};

use crate::{error::DvpError, instructions::shared::u64_right_align};

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SettleDvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

/// The settle proof's single public input. The program supplies every
/// element: `private_tx_hash` from the SPP transact it CPIs, `creator_id`,
/// `dvp_id` and `current_timestamp` from the instruction data, the owner hash
/// from the PDA it derives, and `settlement_authority_id` from the signer.
pub struct SettleDvpPublicInput<'a> {
    pub private_tx_hash: &'a [u8; 32],
    /// `solana_owner_identity(creator)`.
    pub creator_id: &'a [u8; 32],
    pub dvp_id: &'a [u8; 32],
    /// `Poseidon(solana_owner_identity(dvp_authority), ZERO_NULLIFIER_PUBKEY)`.
    pub dvp_authority_owner_hash: &'a [u8; 32],
    /// `solana_owner_identity(settlement_authority)`.
    pub settlement_authority_id: &'a [u8; 32],
    /// The instruction's timestamp, already checked against the clock with
    /// `check_settle_timestamp`. It enters the hash as a right-aligned u64,
    /// so a negative value is refused (D5).
    pub current_timestamp: i64,
}

impl SettleDvpPublicInput<'_> {
    pub fn hash(&self) -> Result<[u8; 32], ProgramError> {
        let current_timestamp =
            u64::try_from(self.current_timestamp).map_err(|_| DvpError::TimestampOutOfWindow)?;
        Poseidon::hashv(&[
            self.private_tx_hash.as_slice(),
            self.creator_id.as_slice(),
            self.dvp_id.as_slice(),
            self.dvp_authority_owner_hash.as_slice(),
            self.settlement_authority_id.as_slice(),
            u64_right_align(current_timestamp).as_slice(),
        ])
        .map_err(|_| DvpError::HashingFailed.into())
    }
}

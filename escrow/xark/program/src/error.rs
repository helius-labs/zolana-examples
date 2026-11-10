use solana_program_error::ProgramError;
use spr_hasher::HasherError;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[repr(u32)]
pub enum TimelockEscrowError {
    #[error("proof verification failed")]
    ProofVerificationFailed = 9000,
    #[error("instruction data is invalid")]
    InvalidInstructionData = 9001,
    #[error("shielded-pool program account is invalid")]
    InvalidShieldedPoolProgram = 9002,
    #[error("escrow-authority account is missing from the transact account list")]
    MissingEscrowAuthority = 9003,
    #[error("hashing failed")]
    HashingFailed = 9004,
    #[error("escrow has not yet unlocked")]
    NotYetUnlocked = 9005,
    #[error("authority bump does not derive the creator's escrow-authority PDA")]
    InvalidEscrowAuthorityBump = 9006,
}

impl From<TimelockEscrowError> for ProgramError {
    fn from(error: TimelockEscrowError) -> Self {
        ProgramError::Custom(error as u32)
    }
}

impl From<HasherError> for TimelockEscrowError {
    fn from(_: HasherError) -> Self {
        Self::HashingFailed
    }
}

impl From<spr_program::verifier::VerifyError> for TimelockEscrowError {
    fn from(error: spr_program::verifier::VerifyError) -> Self {
        match error {
            spr_program::verifier::VerifyError::PublicInputs => Self::HashingFailed,
            spr_program::verifier::VerifyError::Proof => Self::ProofVerificationFailed,
        }
    }
}

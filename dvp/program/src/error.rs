use solana_program_error::ProgramError;
use thiserror::Error;
use zolana_hasher::HasherError;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[repr(u32)]
pub enum DvpError {
    #[error("proof verification failed")]
    ProofVerificationFailed = 10000,
    #[error("instruction data is invalid")]
    InvalidInstructionData = 10001,
    #[error("shielded-pool program account is invalid")]
    InvalidShieldedPoolProgram = 10002,
    #[error("dvp-authority account is missing from the transact account list")]
    MissingDvpAuthority = 10003,
    #[error("dvp authority does not derive from the creator, dvp id and bump")]
    InvalidDvpAuthority = 10004,
    #[error("hashing failed")]
    HashingFailed = 10005,
    #[error("settlement timestamp is outside the time window")]
    TimestampOutOfWindow = 10006,
}

impl From<DvpError> for ProgramError {
    fn from(error: DvpError) -> Self {
        ProgramError::Custom(error as u32)
    }
}

impl From<HasherError> for DvpError {
    fn from(_: HasherError) -> Self {
        Self::HashingFailed
    }
}

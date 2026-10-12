use pinocchio::error::ProgramError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum StakingError {
    InvalidInstructionData = 15000,
    InvalidAccount,
    InvalidSeeds,
    InvalidConfig,
    InvalidCrank,
    InvalidEpoch,
    SubmissionsClosed,
    SubmissionsOpen,
    EpochLocked,
    EpochUnlocked,
    SettlementExpired,
    SettlementNotExpired,
    EpochFull,
    InvalidChain,
    ChainExecuted,
    BatchExists,
    BatchMissing,
    BatchFull,
    InvalidOutput,
    InvalidCiphertext,
    InvalidReset,
    UnresolvedIntents,
    OutstandingBatches,
    Overflow,
    HashingFailed,
    ProofVerificationFailed,
    InvalidPool,
    InvalidTree,
    InvalidRecipient,
}

impl From<StakingError> for ProgramError {
    fn from(error: StakingError) -> Self {
        Self::Custom(error as u32)
    }
}

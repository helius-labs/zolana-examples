use light_program_profiler::profile;
use pinocchio::{AccountView, Address, ProgramResult};
use wincode::{SchemaRead, SchemaWrite};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    instructions::abort_dvp::{process_abort_dvp, AbortDvpProof},
    ABORT_MODE_REJECT,
};

/// reject_dvp instruction data. Its wire layout is the shared processor's
/// private `AbortDvpIxData`.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct RejectDvpIxData {
    /// The terms' creator. With `dvp_id` and `bump` it seeds the PDA; the
    /// reject proof checks it against the terms.
    pub creator: Address,
    pub dvp_id: [u8; 32],
    /// Canonical bump of `dvp_authority` (`find_program_address`).
    pub bump: u8,
    pub reject_proof: AbortDvpProof,
    pub spp: TransactIxData,
}

/// A party rejects the DvP and refunds every deposit (see
/// `process_abort_dvp`). The signer account is the seller or the buyer; the
/// abort proof runs in `ABORT_MODE_REJECT`, which requires the signer to be
/// one of the terms' parties. No expiry check (per spec).
#[inline(never)]
#[profile]
pub fn process_reject_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    process_abort_dvp(accounts, data, ABORT_MODE_REJECT)
}

use light_program_profiler::profile;
use pinocchio::{AccountView, Address, ProgramResult};
use wincode::{SchemaRead, SchemaWrite};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    instructions::abort_dvp::{process_abort_dvp, AbortDvpProof},
    ABORT_MODE_CANCEL,
};

/// 1. Cancel the DvP and refund included deposits.
#[inline(never)]
#[profile]
pub fn process_cancel_dvp_ix(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    // 1. Cancel the DvP and refund included deposits.
    process_abort_dvp(accounts, data, ABORT_MODE_CANCEL)
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CancelDvpIxData {
    pub creator: Address,
    pub dvp_id: [u8; 32],
    pub bump: u8,
    pub cancel_proof: AbortDvpProof,
    pub spp: TransactIxData,
}

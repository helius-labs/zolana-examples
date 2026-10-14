pub mod error;
pub mod instructions;
pub mod verifying_keys;

use pinocchio::{address::address_eq, error::ProgramError, AccountView, Address, ProgramResult};

use crate::instructions::{create_dvp::process_create_dvp_ix, reclaim_dvp::process_reclaim_dvp_ix};

/// Instruction discriminators, matching the reference DvP program.
pub mod tag {
    pub const CREATE_DVP: u8 = 0;
    pub const RECLAIM_DVP: u8 = 1;
    pub const SETTLE_DVP: u8 = 2;
    pub const CANCEL_DVP: u8 = 3;
    pub const REJECT_DVP: u8 = 4;
}

/// First seed of the per-DvP authority PDA `[b"dvp", creator, dvp_id, [bump]]`.
pub const DVP_AUTHORITY_PDA_SEED: &[u8] = b"dvp";

/// Domain tags of the create, settlement and abort blinding seeds
/// (`Poseidon(tag, terms.blinding)`).
pub const CREATE_TAG: u32 = u32::from_be_bytes(*b"DVPC");
pub const SETTLEMENT_TAG: u32 = u32::from_be_bytes(*b"DVPS");
pub const ABORT_TAG: u32 = u32::from_be_bytes(*b"DVPA");

/// Seconds a settlement's `current_timestamp` may trail the cluster clock.
pub const TIME_WINDOW: i64 = 24;

/// Tree id the create circuit pins the DvP address slot to (D6): address
/// uniqueness is per (owner, seed, tree), so a fixed tree makes a
/// `(creator, dvp_id)` pair creatable once.
pub const DVP_ADDRESS_TREE_ID: u16 = 0;

/// Public `mode` of the shared abort circuit, set from the instruction tag.
pub const ABORT_MODE_CANCEL: u8 = 0;
pub const ABORT_MODE_REJECT: u8 = 1;

#[cfg(all(feature = "bpf-entrypoint", not(feature = "no-entrypoint")))]
mod entrypoint {
    pinocchio::entrypoint!(crate::process_instruction);
}

pinocchio::address::declare_id!("8ErdEhvg1NgRzzZRSw3Lx4z4rDEruc6Bjwzn7WVooUMx");

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    if !address_eq(program_id, &crate::ID) {
        return Err(ProgramError::IncorrectProgramId);
    }

    let (ix_tag, ix_data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    match *ix_tag {
        tag::CREATE_DVP => process_create_dvp_ix(accounts, ix_data),
        tag::RECLAIM_DVP => process_reclaim_dvp_ix(accounts, ix_data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

pub mod error;
pub mod instructions;
pub mod math;
pub mod pda;
pub mod state;
pub mod verifying_keys;

pub mod tag {
    pub const CREATE_CONFIG: u8 = 0;
    pub const CREATE_EPOCH: u8 = 1;
    pub const LOCK_EPOCH: u8 = 2;
    pub const UNLOCK_EPOCH: u8 = 3;
    pub const CREATE_BATCH: u8 = 4;
    pub const FILL_BATCH: u8 = 5;
    pub const RESET_BATCH: u8 = 6;
    pub const CLOSE_EPOCH: u8 = 7;
    pub const CREATE_INTENT: u8 = 8;
    pub const CANCEL_INTENT: u8 = 9;
    pub const EXECUTE_BATCH: u8 = 10;
}

#[cfg(all(feature = "bpf-entrypoint", not(feature = "no-entrypoint")))]
mod entrypoint {
    pinocchio::entrypoint!(crate::process_instruction);
}

pub fn process_instruction(
    program_id: &pinocchio::Address,
    accounts: &mut [pinocchio::AccountView],
    data: &[u8],
) -> pinocchio::ProgramResult {
    use instructions::management::*;
    if program_id != &crate::ID {
        return Err(pinocchio::error::ProgramError::IncorrectProgramId);
    }
    let (tag, data) = data
        .split_first()
        .ok_or(error::StakingError::InvalidInstructionData)?;
    match *tag {
        tag::CREATE_CONFIG => process_create_config(accounts, data),
        tag::CREATE_EPOCH => process_create_epoch(accounts, data),
        tag::LOCK_EPOCH => process_lock_epoch(accounts, data),
        tag::UNLOCK_EPOCH => process_unlock_epoch(accounts, data),
        tag::CREATE_BATCH => process_create_batch(accounts, data),
        tag::FILL_BATCH => process_fill_batch(accounts, data),
        tag::RESET_BATCH => process_reset_batch(accounts, data),
        tag::CLOSE_EPOCH => process_close_epoch(accounts, data),
        tag::CREATE_INTENT => instructions::intent::process_create_intent(accounts, data),
        _ => Err(error::StakingError::InvalidInstructionData.into()),
    }
}

pub const N: usize = 16;
pub const M: usize = 282;

pinocchio::address::declare_id!("8mgp9DQDV2DWowsQYn9sM6DsPf1HrcMw1UNcjzHQY61e");

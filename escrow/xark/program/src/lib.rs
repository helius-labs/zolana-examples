#[cfg(all(not(feature = "build-circuits"), not(feature = "insecure-test-setup")))]
compile_error!("the timelock escrow verifying keys require the insecure-test-setup feature");

#[cfg(feature = "circuits")]
pub mod circuits;

pub mod error;
// The Xark circuit build (`--cfg xark`) compiles only the circuits: the
// instructions and the generated verifiers they call are the Solana program.
#[cfg(all(not(feature = "build-circuits"), not(xark)))]
pub mod instructions;
#[cfg(all(not(feature = "build-circuits"), not(xark)))]
#[rustfmt::skip] // Keep the generator's exact output for `zk compile --check`.
pub mod verifier;

#[cfg(all(not(feature = "build-circuits"), not(xark)))]
use pinocchio::{address::address_eq, error::ProgramError, AccountView, Address, ProgramResult};

#[cfg(all(not(feature = "build-circuits"), not(xark)))]
use crate::instructions::{process_escrow_ix, process_withdraw_ix};

pub mod tag {
    pub const ESCROW: u8 = 0;
    pub const WITHDRAW: u8 = 1;
}

pub const ESCROW_AUTHORITY_PDA_SEED: &[u8] = b"escrow_authority";

#[cfg(all(feature = "bpf-entrypoint", not(feature = "build-circuits"), not(xark)))]
mod entrypoint {
    pinocchio::entrypoint!(crate::process_instruction);
}

pinocchio::address::declare_id!("42uWG786CJAdtdZy5KEEtE7QCuSHjVFu5ZAq32Z41hDD");

#[cfg(all(not(feature = "build-circuits"), not(xark)))]
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
        tag::ESCROW => process_escrow_ix(accounts, ix_data),
        tag::WITHDRAW => process_withdraw_ix(accounts, ix_data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

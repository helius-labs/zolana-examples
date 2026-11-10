#[cfg(all(not(feature = "build-circuits"), not(feature = "insecure-test-setup")))]
compile_error!("the swap verifying keys require the insecure-test-setup feature");

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
use crate::instructions::{process_cancel_ix, process_make_ix, process_take_ix};

pub mod tag {
    pub const MAKE: u8 = 2;
    pub const TAKE: u8 = 3;
    pub const CANCEL: u8 = 4;
}

pub const ORDER_AUTHORITY_PDA_SEED: &[u8] = b"order_authority";

/// The seeds of a maker's order-authority PDA, `[ORDER_AUTHORITY_PDA_SEED,
/// maker]`. The PDA owns the maker's order UTXOs, so a maker finds them by
/// deriving it.
pub fn order_authority_seeds(maker: &[u8; 32]) -> [&[u8]; 2] {
    [ORDER_AUTHORITY_PDA_SEED, maker]
}

/// Domain of the take transaction's blinding seed,
/// `Poseidon(SWTX, order_utxo_blinding)`: the ASCII tag as a big-endian
/// 32-bit integer.
pub const TAKE_BLINDING_DOMAIN: u32 = u32::from_be_bytes(*b"SWTX");

#[cfg(all(feature = "bpf-entrypoint", not(feature = "build-circuits"), not(xark)))]
mod entrypoint {
    pinocchio::entrypoint!(crate::process_instruction);
}

pinocchio::address::declare_id!("F4hjig5RJbXW6vrgYaMcuWzR55kZF6tbKyf8c36nrtvU");

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
        tag::MAKE => process_make_ix(accounts, ix_data),
        tag::TAKE => process_take_ix(accounts, ix_data),
        tag::CANCEL => process_cancel_ix(accounts, ix_data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

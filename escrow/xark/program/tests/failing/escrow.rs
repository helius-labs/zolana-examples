use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use spr_account_checks::error::AccountError;
use spr_test_utils::mollusk::expect_err_exact;
use timelock_escrow_program::{error::TimelockEscrowError, tag};

use crate::common::{escrow_fixture, setup_mollusk};

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    expect_escrow_err(
        |instruction| instruction.data = vec![tag::ESCROW, 1, 2, 3],
        TimelockEscrowError::InvalidInstructionData,
    );
}

#[test]
fn trailing_instruction_data_is_rejected_exactly() {
    expect_escrow_err(
        |instruction| instruction.data.push(0),
        TimelockEscrowError::InvalidInstructionData,
    );
}

#[test]
fn truncated_transact_data_is_rejected_exactly() {
    // The proof parses, but the transact data after it lacks its last byte.
    expect_escrow_err(
        |instruction| {
            instruction.data.pop();
        },
        TimelockEscrowError::InvalidInstructionData,
    );
}

#[test]
fn missing_creator_is_rejected_exactly() {
    expect_escrow_err(
        |instruction| instruction.accounts.clear(),
        AccountError::NotEnoughAccountKeys,
    );
}

#[test]
fn unsigned_creator_is_rejected_exactly() {
    expect_escrow_err(
        |instruction| creator(instruction).is_signer = false,
        AccountError::InvalidSigner,
    );
}

#[test]
fn readonly_creator_is_rejected_exactly() {
    expect_escrow_err(
        |instruction| creator(instruction).is_writable = false,
        AccountError::AccountNotMutable,
    );
}

#[test]
fn invalid_escrow_proof_is_rejected_exactly() {
    expect_escrow_err(|_| {}, TimelockEscrowError::ProofVerificationFailed);
}

fn creator(instruction: &mut Instruction) -> &mut solana_instruction::AccountMeta {
    instruction
        .accounts
        .first_mut()
        .expect("creator account meta")
}

/// Runs the escrow fixture after `edit` and expects exactly `error`.
fn expect_escrow_err(edit: impl FnOnce(&mut Instruction), error: impl Into<ProgramError>) {
    let (mut instruction, accounts) = escrow_fixture();
    edit(&mut instruction);
    expect_err_exact(&setup_mollusk(), &instruction, &accounts, error.into());
}

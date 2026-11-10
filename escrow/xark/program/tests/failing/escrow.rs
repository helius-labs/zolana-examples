use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use spr_account_checks::error::AccountError;
use spr_test_utils::mollusk::expect_err_exact;
use timelock_escrow_program::{error::TimelockEscrowError, tag};

use crate::common::{edit_creator, edit_payer, escrow_fixture, setup_mollusk};

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

/// The creator signs for the program, which derives the escrow-authority
/// PDA and the owner identity from it, even when another key pays.
#[test]
fn unsigned_creator_is_rejected_exactly() {
    expect_escrow_err(
        |instruction| edit_creator(instruction, |meta| meta.is_signer = false),
        AccountError::InvalidSigner,
    );
}

/// A creator that is not the fee payer is read-only; one that also pays is
/// writable. Both reach the proof check.
#[test]
fn readonly_or_writable_creator_reaches_the_proof_check() {
    for writable in [false, true] {
        expect_escrow_err(
            |instruction| edit_creator(instruction, |meta| meta.is_writable = writable),
            TimelockEscrowError::ProofVerificationFailed,
        );
    }
}

/// The fee payer is only the SPP payer: an escrow without a separate fee
/// payer account, where the creator pays, reaches the proof check.
#[test]
fn creator_paying_its_own_escrow_reaches_the_proof_check() {
    expect_escrow_err(
        |instruction| {
            let creator = instruction
                .accounts
                .first()
                .expect("creator account meta")
                .pubkey;
            edit_payer(instruction, |meta| meta.pubkey = creator);
        },
        TimelockEscrowError::ProofVerificationFailed,
    );
}

#[test]
fn invalid_escrow_proof_is_rejected_exactly() {
    expect_escrow_err(|_| {}, TimelockEscrowError::ProofVerificationFailed);
}

/// Runs the escrow fixture after `edit` and expects exactly `error`.
fn expect_escrow_err(edit: impl FnOnce(&mut Instruction), error: impl Into<ProgramError>) {
    let (mut instruction, accounts) = escrow_fixture();
    edit(&mut instruction);
    expect_err_exact(&setup_mollusk(), &instruction, &accounts, error.into());
}

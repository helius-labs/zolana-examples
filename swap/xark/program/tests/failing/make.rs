use solana_program_error::ProgramError;
use spr_account_checks::AccountError;
use spr_test_utils::mollusk::expect_err_exact;
use swap_program::error::SwapError;

use crate::common::{
    fixture, marker, setup_mollusk, transact, valid_marker, wrapper_data, wrapper_data_with,
    Wrapper, ORDER_UTXO_HASH, PAYER,
};

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    let (mut instruction, accounts) = fixture(Wrapper::Make);
    instruction.data = vec![Wrapper::Make.tag(), 1, 2, 3];
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(SwapError::InvalidInstructionData as u32),
    );
}

#[test]
fn missing_accounts_are_rejected_exactly() {
    let (mollusk, program_id) = setup_mollusk();
    let instruction = solana_instruction::Instruction {
        program_id,
        accounts: Vec::new(),
        data: wrapper_data(Wrapper::Make),
    };
    expect_err_exact(
        &mollusk,
        &instruction,
        &[],
        ProgramError::Custom(u32::from(AccountError::NotEnoughAccountKeys)),
    );
}

#[test]
fn readonly_payer_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    let (mut instruction, accounts) = fixture(Wrapper::Make);
    instruction
        .accounts
        .first_mut()
        .expect("payer meta")
        .is_writable = false;
    // Both metas name the payer, so Solana unions their privileges.
    instruction
        .accounts
        .get_mut(1)
        .expect("maker meta")
        .is_writable = false;
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(u32::from(AccountError::AccountNotMutable)),
    );
}

#[test]
fn unsigned_payer_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    let (mut instruction, accounts) = fixture(Wrapper::Make);
    instruction
        .accounts
        .first_mut()
        .expect("payer meta")
        .is_signer = false;
    instruction
        .accounts
        .get_mut(1)
        .expect("maker meta")
        .is_signer = false;
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(u32::from(AccountError::InvalidSigner)),
    );
}

/// The make arguments parse, but the transact data that follows them does
/// not: its last byte is dropped, then a byte is appended.
#[test]
fn malformed_transact_data_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    for edit in [
        |data: &mut Vec<u8>| {
            data.pop();
        },
        |data: &mut Vec<u8>| data.push(0),
    ] {
        let (mut instruction, accounts) = fixture(Wrapper::Make);
        edit(&mut instruction.data);
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(SwapError::InvalidInstructionData as u32),
        );
    }
}

/// The fixture's valid marker passes the marker check and reaches the proof
/// check, which its placeholder proof fails.
#[test]
fn valid_marker_reaches_the_proof_check() {
    let (mollusk, _) = setup_mollusk();
    let (instruction, accounts) = fixture(Wrapper::Make);
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(SwapError::ProofVerificationFailed as u32),
    );
}

/// A marker naming another order UTXO, another maker, or holding no data
/// fails the marker check.
#[test]
fn wrong_marker_data_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    let mut empty = valid_marker();
    empty.data.clear();
    let mut extended = valid_marker();
    extended.data.push(0);
    for message in [
        marker([7; 32], PAYER),
        marker(ORDER_UTXO_HASH, [7; 32]),
        empty,
        extended,
    ] {
        let (mut instruction, accounts) = fixture(Wrapper::Make);
        instruction.data = wrapper_data_with(Wrapper::Make, transact(vec![message]));
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(SwapError::InvalidMarkerData as u32),
        );
    }
}

/// The make carries exactly one message: none, or a valid marker with a
/// second message, fails.
#[test]
fn zero_or_two_messages_are_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    for messages in [Vec::new(), vec![valid_marker(), valid_marker()]] {
        let (mut instruction, accounts) = fixture(Wrapper::Make);
        instruction.data = wrapper_data_with(Wrapper::Make, transact(messages));
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(SwapError::InvalidMarkerMessage as u32),
        );
    }
}

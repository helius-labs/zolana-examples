use mollusk_svm::Mollusk;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use spr_test_utils::mollusk::expect_err_exact;
use timelock_escrow_program::tag;

use crate::common::setup_mollusk;

fn expect_invalid_instruction_data(mollusk: &Mollusk, data: Vec<u8>) {
    let instruction = Instruction {
        program_id: timelock_escrow_program::ID,
        accounts: Vec::new(),
        data,
    };
    expect_err_exact(
        mollusk,
        &instruction,
        &[],
        ProgramError::InvalidInstructionData,
    );
}

#[test]
fn missing_instruction_tag_is_rejected_exactly() {
    expect_invalid_instruction_data(&setup_mollusk(), Vec::new());
}

#[test]
fn every_unassigned_instruction_tag_is_rejected_exactly() {
    // Both tags are routed; every tag past the last one is unknown.
    let mollusk = setup_mollusk();
    for ix_tag in (tag::WITHDRAW + 1)..=u8::MAX {
        expect_invalid_instruction_data(&mollusk, vec![ix_tag]);
    }
}

use dvp_program::tag;
use mollusk_svm::Mollusk;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use zolana_test_utils::mollusk::expect_err_exact;

use crate::common::{program_id, setup_mollusk};

fn expect_invalid_instruction_data(mollusk: &Mollusk, data: Vec<u8>) {
    let instruction = Instruction {
        program_id: program_id(),
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
    // All five tags are routed; every tag past the last one is unknown.
    let mollusk = setup_mollusk();
    for ix_tag in (tag::REJECT_DVP + 1)..=u8::MAX {
        expect_invalid_instruction_data(&mollusk, vec![ix_tag]);
    }
}

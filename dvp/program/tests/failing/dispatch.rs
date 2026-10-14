use dvp_program::tag;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use zolana_test_utils::mollusk::expect_err_exact;

use crate::common::{program_id, setup_mollusk};

fn expect_invalid_instruction_data(data: Vec<u8>) {
    let instruction = Instruction {
        program_id: program_id(),
        accounts: Vec::new(),
        data,
    };
    expect_err_exact(
        &setup_mollusk(),
        &instruction,
        &[],
        ProgramError::InvalidInstructionData,
    );
}

#[test]
fn missing_instruction_tag_is_rejected_exactly() {
    expect_invalid_instruction_data(Vec::new());
}

#[test]
fn unknown_instruction_tag_is_rejected_exactly() {
    expect_invalid_instruction_data(vec![0xff]);
}

#[test]
fn unrouted_instruction_tags_are_rejected_exactly() {
    for ix_tag in [tag::CANCEL_DVP, tag::REJECT_DVP] {
        expect_invalid_instruction_data(vec![ix_tag]);
    }
}

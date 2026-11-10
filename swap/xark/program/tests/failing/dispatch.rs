use mollusk_svm::result::ProgramResult;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use spr_test_utils::mollusk::expect_err_exact;
use swap_program::error::SwapError;

use crate::common::{fixture, fixture_transact, setup_mollusk, wrapper_data_with_bump, Wrapper};

#[test]
fn missing_instruction_tag_is_rejected_exactly() {
    let (mollusk, program_id) = setup_mollusk();
    let instruction = Instruction {
        program_id,
        accounts: Vec::new(),
        data: Vec::new(),
    };
    expect_err_exact(
        &mollusk,
        &instruction,
        &[],
        ProgramError::InvalidInstructionData,
    );
}

#[test]
fn unknown_instruction_tag_is_rejected_exactly() {
    let (mollusk, program_id) = setup_mollusk();
    let instruction = Instruction {
        program_id,
        accounts: Vec::new(),
        data: vec![0xff],
    };
    expect_err_exact(
        &mollusk,
        &instruction,
        &[],
        ProgramError::InvalidInstructionData,
    );
}

/// A bump that derives no program address fails right after decoding, before
/// the marker, window and proof checks, in every wrapper.
#[test]
fn invalid_authority_bump_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    for wrapper in [Wrapper::Make, Wrapper::Take, Wrapper::Cancel] {
        let (mut instruction, accounts) = fixture(wrapper);
        instruction.data =
            wrapper_data_with_bump(wrapper, fixture_transact(wrapper), wrapper.invalid_bump());
        let result = mollusk.process_instruction(&instruction, &accounts);
        assert_eq!(
            result.program_result,
            ProgramResult::Failure(ProgramError::Custom(
                SwapError::InvalidOrderAuthorityBump as u32
            )),
            "{wrapper:?} must reject an invalid authority bump"
        );
    }
}

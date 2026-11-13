use mollusk_svm::result::ProgramResult;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use swap_program::error::SwapError;

use crate::common::{
    expect_err_exact, fixture, fixture_transact, invalid_bump, setup_mollusk,
    wrapper_data_with_bump, Wrapper,
};

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
        instruction.data = wrapper_data_with_bump(
            wrapper,
            fixture_transact(wrapper),
            invalid_bump(wrapper.maker()),
        );
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

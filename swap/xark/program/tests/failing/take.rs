use solana_program_error::ProgramError;
use spr_interface::instruction::instruction_data::transact::InputUtxo;
use spr_test_utils::mollusk::expect_err_exact;
use swap_program::error::SwapError;

use crate::common::{fixture, setup_mollusk, transact, wrapper_data_with, Wrapper};

#[test]
fn expired_take_is_rejected_exactly() {
    let (mut mollusk, _) = setup_mollusk();
    let (mut instruction, accounts) = fixture(Wrapper::Take);
    // Bind the relayer deadline below the warped clock so the window check is
    // the branch that fires.
    let mut data = transact(Vec::new());
    data.expiry_unix_ts = 5;
    instruction.data = wrapper_data_with(Wrapper::Take, data);
    mollusk.sysvars.clock.unix_timestamp = 6;
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(SwapError::Expired as u32),
    );
}

#[test]
fn oversized_take_private_tx_hash_fails_hashing_exactly() {
    let (mollusk, _) = setup_mollusk();
    let (mut instruction, accounts) = fixture(Wrapper::Take);
    // 0xFF-filled bytes exceed the BN254 modulus, so the public-input Poseidon
    // hash fails before proof verification.
    let mut data = transact(Vec::new());
    data.inputs.push(InputUtxo {
        nullifier_hash: [1; 32],
        tree_index: 0,
    });
    data.private_tx_hash = [0xFF; 32];
    instruction.data = wrapper_data_with(Wrapper::Take, data);
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(SwapError::HashingFailed as u32),
    );
}

#[test]
fn missing_first_nullifier_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    let (instruction, accounts) = fixture(Wrapper::Take);
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(SwapError::InvalidInstructionData as u32),
    );
}

#[test]
fn truncated_instruction_data_is_rejected_exactly() {
    let (mollusk, _) = setup_mollusk();
    let (mut instruction, accounts) = fixture(Wrapper::Take);
    instruction.data = vec![Wrapper::Take.tag(), 1, 2, 3];
    expect_err_exact(
        &mollusk,
        &instruction,
        &accounts,
        ProgramError::Custom(SwapError::InvalidInstructionData as u32),
    );
}

/// The take arguments parse, but the transact data that follows them does
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
        let (mut instruction, accounts) = fixture(Wrapper::Take);
        edit(&mut instruction.data);
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(SwapError::InvalidInstructionData as u32),
        );
    }
}

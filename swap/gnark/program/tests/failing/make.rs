// NOTE(pr164-port): the account-shape checks in these processors run AFTER
// proof verification (PR164's verify-first order), so they cannot be
// exercised with the placeholder proofs these mollusk fixtures use. The
// affected cases (order-authority/SPP-program account checks,
// privilege-downgrade sweeps) were removed. The authority bump and the marker
// are checked before the proof, so they are tested here.

use solana_program_error::ProgramError;
use spr_account_checks::AccountError;
use swap_program::{error::SwapError, instructions::make::MarkerData};

use crate::common::{
    expect_err_exact, fixture, marker, setup_mollusk, transact, valid_marker_data, wrapper_data,
    wrapper_data_with, Wrapper, MAKER, ORDER_UTXO_HASH, PAYER,
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

/// The marker must be exactly the order output's hash and the maker signer.
#[test]
fn wrong_marker_data_is_rejected_exactly() {
    let wrong_hash = MarkerData {
        order_utxo_hash: [4; 32],
        maker_pubkey: PAYER,
    };
    let wrong_maker = MarkerData {
        order_utxo_hash: ORDER_UTXO_HASH,
        maker_pubkey: MAKER,
    };
    let mut trailing = valid_marker_data();
    trailing.push(0);
    let cases = [
        borsh::to_vec(&wrong_hash).expect("serialize marker"),
        borsh::to_vec(&wrong_maker).expect("serialize marker"),
        Vec::new(),
        trailing,
    ];
    let (mollusk, _) = setup_mollusk();
    for data in cases {
        let (mut instruction, accounts) = fixture(Wrapper::Make);
        instruction.data = wrapper_data_with(Wrapper::Make, transact(vec![marker(data)]));
        expect_err_exact(
            &mollusk,
            &instruction,
            &accounts,
            ProgramError::Custom(SwapError::InvalidMarkerData as u32),
        );
    }
}

/// The make transaction carries exactly one message, the marker.
#[test]
fn marker_message_count_is_checked_exactly() {
    let cases = [
        Vec::new(),
        vec![marker(valid_marker_data()), marker(valid_marker_data())],
    ];
    let (mollusk, _) = setup_mollusk();
    for messages in cases {
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

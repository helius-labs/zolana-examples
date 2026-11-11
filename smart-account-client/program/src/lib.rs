//! A stripped-down user registry that keeps only `set_data`: one data record
//! per owner, at the PDA `[b"data", owner]`, that only the owner can replace.
//! The record account holds the data as a Borsh `Vec<u8>` (a u32 length,
//! then the bytes). The smart account client keeps the vault's current key
//! record here; earlier data survives only in the `set_data` instructions
//! that wrote it.

use pinocchio::{
    cpi::{invoke, invoke_signed, Seed, Signer},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
    sysvars::{rent::Rent, Sysvar},
    AccountView, Address, ProgramResult, Resize,
};

const DATA_SEED: &[u8] = b"data";
/// `set_data` instruction tag; the payload is a Borsh `Vec<u8>`.
const SET_DATA: u8 = 0;
const SYSTEM_PROGRAM_ID: Address = Address::new_from_array([0u8; 32]);

#[cfg(feature = "bpf-entrypoint")]
pinocchio::entrypoint!(process_instruction);

/// `set_data`: replaces `owner`'s data with the payload, creating the record
/// on the first write and resizing it on later ones.
///
/// Accounts: writable record, owner signer, writable rent payer signer,
/// system program. The rent payer covers the rent the record is missing;
/// lamports freed by shrinking stay in the record.
pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let Some((&SET_DATA, payload)) = instruction_data.split_first() else {
        return Err(ProgramError::InvalidInstructionData);
    };
    let (length, data) = payload
        .split_first_chunk::<4>()
        .ok_or(ProgramError::InvalidInstructionData)?;
    if u32::from_le_bytes(*length) as usize != data.len() {
        return Err(ProgramError::InvalidInstructionData);
    }

    let [record, owner, rent_payer, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !owner.is_signer() || !rent_payer.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !record.is_writable() {
        return Err(ProgramError::InvalidAccountData);
    }
    if system_program.address() != &SYSTEM_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let (expected, bump) =
        Address::find_program_address(&[DATA_SEED, owner.address().as_ref()], program_id);
    if record.address() != &expected {
        return Err(ProgramError::InvalidSeeds);
    }

    // The record stores the payload as is: the u32 length, then the data.
    let size = payload.len();
    let required = Rent::get()?.try_minimum_balance(size)?;
    let top_up = required.saturating_sub(record.lamports());
    if top_up > 0 {
        system_transfer(rent_payer, record, top_up)?;
    }
    if record.owned_by(program_id) {
        record.resize(size)?;
    } else {
        create_record(record, owner.address(), bump, size, program_id)?;
    }
    record.try_borrow_mut()?.copy_from_slice(payload);
    Ok(())
}

/// Allocates `size` bytes for the record PDA and assigns it to this program.
/// Allocate and assign, unlike create_account, accept a record that already
/// holds the lamports `system_transfer` sent it.
fn create_record(
    record: &AccountView,
    owner: &Address,
    bump: u8,
    size: usize,
    program_id: &Address,
) -> ProgramResult {
    let bump_seed = [bump];
    let seeds = [
        Seed::from(DATA_SEED),
        Seed::from(owner.as_ref()),
        Seed::from(&bump_seed[..]),
    ];
    let signers = [Signer::from(&seeds[..])];
    let metas = [InstructionAccount::writable_signer(record.address())];

    let mut allocate_data = [0u8; 12];
    allocate_data[0] = 8;
    allocate_data[4..12].copy_from_slice(&(size as u64).to_le_bytes());
    let allocate = InstructionView {
        program_id: &SYSTEM_PROGRAM_ID,
        accounts: &metas,
        data: &allocate_data,
    };
    invoke_signed::<1, _>(&allocate, &[record], &signers)?;

    let mut assign_data = [0u8; 36];
    assign_data[0] = 1;
    assign_data[4..36].copy_from_slice(program_id.as_ref());
    let assign = InstructionView {
        program_id: &SYSTEM_PROGRAM_ID,
        accounts: &metas,
        data: &assign_data,
    };
    invoke_signed::<1, _>(&assign, &[record], &signers)
}

fn system_transfer(from: &AccountView, to: &AccountView, lamports: u64) -> ProgramResult {
    let mut data = [0u8; 12];
    data[0] = 2;
    data[4..12].copy_from_slice(&lamports.to_le_bytes());
    let metas = [
        InstructionAccount::writable_signer(from.address()),
        InstructionAccount::writable(to.address()),
    ];
    let instruction = InstructionView {
        program_id: &SYSTEM_PROGRAM_ID,
        accounts: &metas,
        data: &data,
    };
    invoke::<2, _>(&instruction, &[from, to])
}

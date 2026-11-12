//! Append-only uploads, published atomically through an owner-authorized head.
//! Publishing a replacement atomically closes its predecessor and refunds its
//! rent to whoever paid for that upload.

use pinocchio::{
    cpi::{invoke, invoke_signed, Seed, Signer},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
    sysvars::{rent::Rent, Sysvar},
    AccountView, Address, ProgramResult, Resize,
};
use solana_sha256_hasher::hashv;

pub mod state;
use state::{
    decode_head, encode_head, read, BufferHeader, Head, APPLY_ARGUMENTS_LEN, BUFFER_HEADER_LEN,
    CHUNK_LEN, HEAD_LEN, HEAD_PDA_SEED, MAX_RECORD_LEN, STORE_ARGUMENTS_LEN, ZERO_ADDRESS,
};

pub const STORE: u8 = 1;
pub const APPLY: u8 = 2;
pub const CLOSE: u8 = 3;
const SYSTEM_PROGRAM_ID: Address = Address::new_from_array([0; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum StorageError {
    Sealed = 7000,
    InvalidOffset,
    InvalidLength,
    HashMismatch,
    StaleHead,
    MetadataMismatch,
    PayerMismatch,
    RefundMismatch,
}

impl From<StorageError> for ProgramError {
    fn from(error: StorageError) -> Self {
        Self::Custom(error as u32)
    }
}

#[cfg(feature = "bpf-entrypoint")]
pinocchio::entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let (tag, arguments) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        STORE => process_store(program_id, accounts, arguments),
        APPLY => process_apply(program_id, accounts, arguments),
        CLOSE if arguments.is_empty() => process_close(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn process_store(
    program_id: &Address,
    accounts: &mut [AccountView],
    arguments: &[u8],
) -> ProgramResult {
    let (mut fields, chunk) = arguments
        .split_at_checked(STORE_ARGUMENTS_LEN)
        .ok_or(ProgramError::InvalidInstructionData)?;
    let owner = Address::new_from_array(read(&mut fields)?);
    let previous = Address::new_from_array(read(&mut fields)?);
    let total_length = u32::from_le_bytes(read(&mut fields)?);
    let offset = u32::from_le_bytes(read(&mut fields)?) as usize;
    if chunk.is_empty()
        || chunk.len() > CHUNK_LEN
        || total_length == 0
        || total_length as usize > MAX_RECORD_LEN
    {
        return Err(StorageError::InvalidLength.into());
    }
    let [buffer_account, payer_account, system_program_account, refund_account, ..] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !buffer_account.is_signer() || !payer_account.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    require_writable(buffer_account)?;
    require_payer(payer_account, system_program_account)?;
    require_distinct(buffer_account, payer_account)?;
    let initialized = buffer_account.owned_by(program_id);
    let mut header = if initialized {
        load_buffer(buffer_account, program_id)?
    } else {
        require_empty_system_account(buffer_account)?;
        require_refund_destination(refund_account, buffer_account)?;
        BufferHeader {
            sealed: false,
            owner,
            payer: *payer_account.address(),
            refund: *refund_account.address(),
            previous,
            total_length,
            data_hash: [0; 32],
        }
    };
    if header.sealed {
        return Err(StorageError::Sealed.into());
    }
    if header.owner != owner
        || header.previous != previous
        || header.total_length != total_length
        || &header.refund != refund_account.address()
    {
        return Err(StorageError::MetadataMismatch.into());
    }
    // One payer funds the whole buffer, so one account receives every refund.
    if &header.payer != payer_account.address() {
        return Err(StorageError::PayerMismatch.into());
    }
    let written = if initialized {
        buffer_account
            .data_len()
            .checked_sub(BUFFER_HEADER_LEN)
            .ok_or(ProgramError::InvalidAccountData)?
    } else {
        0
    };
    if offset != written {
        return Err(StorageError::InvalidOffset.into());
    }
    let end = offset
        .checked_add(chunk.len())
        .ok_or(StorageError::InvalidLength)?;
    // Only the final chunk may be short, so the bytes alone fix the hash.
    if end > total_length as usize || (chunk.len() != CHUNK_LEN && end != total_length as usize) {
        return Err(StorageError::InvalidLength.into());
    }
    let start = BUFFER_HEADER_LEN
        .checked_add(offset)
        .ok_or(StorageError::InvalidLength)?;
    let size = BUFFER_HEADER_LEN
        .checked_add(end)
        .ok_or(StorageError::InvalidLength)?;
    header.data_hash = hashv(&[&header.data_hash, chunk]).to_bytes();
    top_up_rent(payer_account, buffer_account, size)?;
    if initialized {
        buffer_account.resize(size)?;
    } else {
        allocate_and_assign(buffer_account, size, program_id, &[])?;
    }
    let mut buffer_bytes = buffer_account.try_borrow_mut()?;
    header.encode(&mut buffer_bytes)?;
    buffer_bytes
        .get_mut(start..size)
        .ok_or(ProgramError::InvalidAccountData)?
        .copy_from_slice(chunk);
    Ok(())
}

fn process_apply(
    program_id: &Address,
    accounts: &mut [AccountView],
    mut arguments: &[u8],
) -> ProgramResult {
    if arguments.len() != APPLY_ARGUMENTS_LEN {
        return Err(ProgramError::InvalidInstructionData);
    }
    let expected_previous = Address::new_from_array(read(&mut arguments)?);
    let expected_revision = u64::from_le_bytes(read(&mut arguments)?);
    let approved_hash: [u8; 32] = read(&mut arguments)?;
    let [head_account, buffer_account, owner_account, payer_account, system_program_account, previous_account, refund_account, ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !owner_account.is_signer() || !payer_account.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    require_writable(head_account)?;
    require_writable(buffer_account)?;
    require_payer(payer_account, system_program_account)?;
    require_distinct(head_account, buffer_account)?;
    require_distinct(head_account, payer_account)?;
    require_distinct(buffer_account, payer_account)?;
    let (head_pda, bump) = Address::find_program_address(
        &[HEAD_PDA_SEED, owner_account.address().as_ref()],
        program_id,
    );
    if head_account.address() != &head_pda {
        return Err(ProgramError::InvalidSeeds);
    }
    let mut header = load_buffer(buffer_account, program_id)?;
    if header.sealed {
        return Err(StorageError::Sealed.into());
    }
    if &header.owner != owner_account.address() {
        return Err(ProgramError::IllegalOwner);
    }
    let expected_size = BUFFER_HEADER_LEN
        .checked_add(header.total_length as usize)
        .ok_or(StorageError::InvalidLength)?;
    if buffer_account.data_len() != expected_size {
        return Err(StorageError::InvalidLength.into());
    }
    if approved_hash != header.data_hash {
        return Err(StorageError::HashMismatch.into());
    }
    // A head that must refund itself could never be replaced again.
    if &header.refund == head_account.address() {
        return Err(ProgramError::InvalidArgument);
    }
    let initialized = head_account.owned_by(program_id);
    let current = if initialized {
        decode_head(&head_account.try_borrow()?)?
    } else {
        require_empty_system_account(head_account)?;
        Head {
            current: ZERO_ADDRESS,
            revision: 0,
        }
    };
    // Revisions prevent stale approvals succeeding after a closed address is reused.
    if header.previous != expected_previous
        || current.current != expected_previous
        || current.revision != expected_revision
    {
        return Err(StorageError::StaleHead.into());
    }
    if previous_account.address() != &expected_previous {
        return Err(ProgramError::InvalidArgument);
    }
    // A destination that became a program cannot be credited; its refund goes
    // to the apply payer so the record stays replaceable.
    let mut refund_to_payer = false;
    if initialized {
        require_writable(previous_account)?;
        for other in [&*head_account, &*buffer_account, &*payer_account] {
            require_distinct(previous_account, other)?;
        }
        for other in [&*head_account, &*buffer_account, &*previous_account] {
            require_distinct(refund_account, other)?;
        }
        let previous_header = load_buffer(previous_account, program_id)?;
        if !previous_header.sealed {
            return Err(ProgramError::InvalidAccountData);
        }
        if &previous_header.owner != owner_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if &previous_header.refund != refund_account.address() {
            return Err(StorageError::RefundMismatch.into());
        }
        refund_to_payer = refund_account.executable();
        if !refund_to_payer {
            require_writable(refund_account)?;
        }
    } else if refund_account.address() != &SYSTEM_PROGRAM_ID {
        // The first publication closes nothing; both slots hold the System Program.
        return Err(ProgramError::InvalidArgument);
    }
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    if !initialized {
        top_up_rent(payer_account, head_account, HEAD_LEN)?;
        let bump_seed = [bump];
        let seeds = [
            Seed::from(HEAD_PDA_SEED),
            Seed::from(owner_account.address().as_ref()),
            Seed::from(&bump_seed[..]),
        ];
        allocate_and_assign(
            head_account,
            HEAD_LEN,
            program_id,
            &[Signer::from(&seeds[..])],
        )?;
    }
    header.sealed = true;
    header.encode(&mut buffer_account.try_borrow_mut()?)?;
    encode_head(
        &mut head_account.try_borrow_mut()?,
        buffer_account.address(),
        next_revision,
    )?;
    if initialized {
        let recipient_account = if refund_to_payer {
            payer_account
        } else {
            refund_account
        };
        close_account(previous_account, recipient_account)?;
    }
    Ok(())
}

/// Cancels an unpublished upload: its rent payer signs and the rent goes to
/// its refund destination.
fn process_close(program_id: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    let [buffer_account, payer_account, refund_account, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !payer_account.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    require_writable(buffer_account)?;
    require_writable(refund_account)?;
    require_distinct(buffer_account, refund_account)?;
    let header = load_buffer(buffer_account, program_id)?;
    if header.sealed {
        return Err(StorageError::Sealed.into());
    }
    if &header.payer != payer_account.address() {
        return Err(StorageError::PayerMismatch.into());
    }
    if &header.refund != refund_account.address() {
        return Err(StorageError::RefundMismatch.into());
    }
    close_account(buffer_account, refund_account)
}

/// Moves every lamport to `recipient_account`, then hands the account back to
/// the System Program with no data, so later instructions in the same
/// transaction cannot load it either.
fn close_account(account: &mut AccountView, recipient_account: &mut AccountView) -> ProgramResult {
    let refund = recipient_account
        .lamports()
        .checked_add(account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    recipient_account.set_lamports(refund);
    account.close()
}

fn load_buffer(account: &AccountView, program_id: &Address) -> Result<BufferHeader, ProgramError> {
    if !account.owned_by(program_id) || account.executable() {
        return Err(ProgramError::IllegalOwner);
    }
    BufferHeader::decode(&account.try_borrow()?)
}

fn require_writable(account: &AccountView) -> ProgramResult {
    if !account.is_writable() {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

fn require_distinct(first: &AccountView, second: &AccountView) -> ProgramResult {
    if first.address() == second.address() {
        return Err(ProgramError::InvalidArgument);
    }
    Ok(())
}

/// The refund destination is fixed at the first `store`. It must be a
/// non-executable System Program account other than the buffer, so it can be
/// credited when the buffer closes.
fn require_refund_destination(
    refund_account: &AccountView,
    buffer_account: &AccountView,
) -> ProgramResult {
    require_distinct(refund_account, buffer_account)?;
    if !refund_account.owned_by(&SYSTEM_PROGRAM_ID) || refund_account.executable() {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

fn require_empty_system_account(account: &AccountView) -> ProgramResult {
    if !account.owned_by(&SYSTEM_PROGRAM_ID) || account.data_len() != 0 || account.executable() {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

fn require_payer(
    payer_account: &AccountView,
    system_program_account: &AccountView,
) -> ProgramResult {
    require_writable(payer_account)?;
    require_empty_system_account(payer_account)?;
    if system_program_account.address() != &SYSTEM_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    Ok(())
}

fn top_up_rent(payer_account: &AccountView, account: &AccountView, size: usize) -> ProgramResult {
    let required = Rent::get()?.try_minimum_balance(size)?;
    let top_up = required.saturating_sub(account.lamports());
    if top_up == 0 {
        return Ok(());
    }
    let mut instruction_data = [0u8; 12];
    instruction_data[0] = 2;
    instruction_data[4..12].copy_from_slice(&top_up.to_le_bytes());
    let metas = [
        InstructionAccount::writable_signer(payer_account.address()),
        InstructionAccount::writable(account.address()),
    ];
    invoke::<2, _>(
        &InstructionView {
            program_id: &SYSTEM_PROGRAM_ID,
            accounts: &metas,
            data: &instruction_data,
        },
        &[payer_account, account],
    )
}

fn allocate_and_assign(
    account: &AccountView,
    size: usize,
    program_id: &Address,
    signers: &[Signer],
) -> ProgramResult {
    let metas = [InstructionAccount::writable_signer(account.address())];
    let mut allocate_data = [0u8; 12];
    allocate_data[0] = 8;
    allocate_data[4..12].copy_from_slice(&(size as u64).to_le_bytes());
    invoke_signed::<1, _>(
        &InstructionView {
            program_id: &SYSTEM_PROGRAM_ID,
            accounts: &metas,
            data: &allocate_data,
        },
        &[account],
        signers,
    )?;
    let mut assign_data = [0u8; 36];
    assign_data[0] = 1;
    assign_data[4..36].copy_from_slice(program_id.as_ref());
    invoke_signed::<1, _>(
        &InstructionView {
            program_id: &SYSTEM_PROGRAM_ID,
            accounts: &metas,
            data: &assign_data,
        },
        &[account],
        signers,
    )
}

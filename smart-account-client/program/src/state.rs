use crate::StorageError;
use pinocchio::{error::ProgramError, Address};

pub const BUFFER_HEADER_LEN: usize = 109;
pub const HEAD_LEN: usize = 48;
pub const MAX_CHUNK_LEN: usize = 1024;
pub const MAX_RECORD_LEN: usize = 10 * 1024 * 1024 - BUFFER_HEADER_LEN;
pub const HEAD_PDA_SEED: &[u8] = b"head";
pub const ZERO_ADDRESS: Address = Address::new_from_array([0; 32]);
const BUFFER_DISCRIMINATOR: &[u8; 8] = b"STORBUF1";
const HEAD_DISCRIMINATOR: &[u8; 8] = b"STORHEAD";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub current: Address,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferHeader {
    pub sealed: bool,
    pub owner: Address,
    pub previous: Address,
    pub total_length: u32,
    pub data_hash: [u8; 32],
}

impl BufferHeader {
    pub fn decode(mut account_bytes: &[u8]) -> Result<Self, ProgramError> {
        if &read::<8>(&mut account_bytes)? != BUFFER_DISCRIMINATOR {
            return Err(ProgramError::InvalidAccountData);
        }
        let sealed = match read::<1>(&mut account_bytes)? {
            [0] => false,
            [1] => true,
            _ => return Err(ProgramError::InvalidAccountData),
        };
        let header = Self {
            sealed,
            owner: Address::new_from_array(read(&mut account_bytes)?),
            previous: Address::new_from_array(read(&mut account_bytes)?),
            total_length: u32::from_le_bytes(read(&mut account_bytes)?),
            data_hash: read(&mut account_bytes)?,
        };
        if header.total_length == 0
            || header.total_length as usize > MAX_RECORD_LEN
            || account_bytes.len() > header.total_length as usize
        {
            return Err(StorageError::InvalidLength.into());
        }
        Ok(header)
    }

    pub fn encode(self, mut header_bytes: &mut [u8]) -> Result<(), ProgramError> {
        write(&mut header_bytes, BUFFER_DISCRIMINATOR)?;
        write(&mut header_bytes, &[u8::from(self.sealed)])?;
        write(&mut header_bytes, self.owner.as_ref())?;
        write(&mut header_bytes, self.previous.as_ref())?;
        write(&mut header_bytes, &self.total_length.to_le_bytes())?;
        write(&mut header_bytes, &self.data_hash)
    }
}

pub fn decode_head(mut account_bytes: &[u8]) -> Result<Head, ProgramError> {
    if account_bytes.len() != HEAD_LEN || &read::<8>(&mut account_bytes)? != HEAD_DISCRIMINATOR {
        return Err(ProgramError::InvalidAccountData);
    }
    let head = Head {
        current: Address::new_from_array(read(&mut account_bytes)?),
        revision: u64::from_le_bytes(read(&mut account_bytes)?),
    };
    if head.current == ZERO_ADDRESS || head.revision == 0 {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(head)
}

pub fn encode_head(
    mut account_bytes: &mut [u8],
    current: &Address,
    revision: u64,
) -> Result<(), ProgramError> {
    write(&mut account_bytes, HEAD_DISCRIMINATOR)?;
    write(&mut account_bytes, current.as_ref())?;
    write(&mut account_bytes, &revision.to_le_bytes())
}

pub fn read<const N: usize>(remaining: &mut &[u8]) -> Result<[u8; N], ProgramError> {
    let (field, rest) = remaining
        .split_first_chunk::<N>()
        .ok_or(ProgramError::InvalidAccountData)?;
    *remaining = rest;
    Ok(*field)
}

fn write(remaining: &mut &mut [u8], field: &[u8]) -> Result<(), ProgramError> {
    let (destination, rest) = core::mem::take(remaining)
        .split_at_mut_checked(field.len())
        .ok_or(ProgramError::InvalidAccountData)?;
    destination.copy_from_slice(field);
    *remaining = rest;
    Ok(())
}

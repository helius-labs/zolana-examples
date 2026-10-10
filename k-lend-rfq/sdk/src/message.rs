//! Decoding of compiled transaction messages back into instructions, used by
//! both parties to inspect a co-signed swap before signing it. Every index is
//! bounds-checked against the static account keys; address lookup tables are
//! not resolved, so an index into one fails like any out-of-range index.

use std::collections::BTreeSet;

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_message::VersionedMessage;
use zolana_interface::{
    instruction::{tag, TransactIxData},
    PROGRAM_ID_PUBKEY,
};

use crate::swap::SwapError;

/// Decompiles every instruction of `message` back into an `Instruction`,
/// resolving account indices against the message's static account keys.
///
/// Errors with `SwapError::UnexpectedTransaction` when a program or account
/// index points past the static account keys.
pub fn instructions(message: &VersionedMessage) -> Result<Vec<Instruction>> {
    let keys = message.static_account_keys();
    let key = |key_index: u8| -> Result<Address> {
        Ok(*keys
            .get(usize::from(key_index))
            .ok_or(SwapError::UnexpectedTransaction)?)
    };
    message
        .instructions()
        .iter()
        .map(|compiled| {
            let accounts = compiled
                .accounts
                .iter()
                .map(|&key_index| {
                    Ok(AccountMeta {
                        pubkey: key(key_index)?,
                        is_signer: message.is_signer(usize::from(key_index)),
                        is_writable: message.is_maybe_writable_with_reserved_addresses(
                            usize::from(key_index),
                            None::<&BTreeSet<Address>>,
                        ),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Instruction {
                program_id: key(compiled.program_id_index)?,
                accounts,
                data: compiled.data.clone(),
            })
        })
        .collect()
}

/// Decodes the `TransactIxData` of a zolana `transact` instruction.
///
/// Errors with `SwapError::UnexpectedTransaction` when `instruction` is not
/// addressed to the zolana program or its tag is not `transact`, and with a
/// decode error when the instruction data does not deserialize.
pub fn transact_data(instruction: &Instruction) -> Result<TransactIxData> {
    if instruction.program_id != PROGRAM_ID_PUBKEY {
        return Err(SwapError::UnexpectedTransaction.into());
    }
    let Some((&tag::TRANSACT, instruction_data)) = instruction.data.split_first() else {
        return Err(SwapError::UnexpectedTransaction.into());
    };
    TransactIxData::deserialize(instruction_data)
        .map_err(|error| anyhow!("decode transact: {error}"))
}

//! The create transaction's terms messages: the plaintext `SwapDvp`
//! encrypted to the seller, the buyer and the settlement authority.

use anyhow::{ensure, Result};
use zolana_interface::instruction::instruction_data::transact::MessageData;
use zolana_keypair::{P256Pubkey, ShieldedAddress, ViewingKey};

use crate::{err, state::SwapDvp};

/// Recipients of the terms messages, in message order.
pub const TERMS_RECIPIENTS: usize = 3;

/// Ciphertext slot of the first terms message (D7). Messages continue the
/// slot numbering after the instruction's outputs; the create transaction
/// publishes one output (the DvP UTXO), its compact padding is omitted.
const FIRST_TERMS_MESSAGE_SLOT: u32 = 1;

/// The Solana pubkeys of the terms recipients, in message order: seller,
/// buyer, settlement authority.
pub(crate) fn terms_recipients(terms: &SwapDvp) -> [solana_address::Address; TERMS_RECIPIENTS] {
    [terms.seller, terms.buyer, terms.settlement_authority]
}

/// One message per recipient `j`, encrypted under the transaction viewing key
/// into slot `1 + j` (D7). `data` is the bare AES-CTR ciphertext: a viewing
/// key prefix would publish the parties' viewing keys. The view tag is the
/// recipient's pubkey, as for a default-ring output.
///
/// Errors when a recipient address is not the pubkey the terms name for it.
pub fn encrypt_terms_messages(
    terms: &SwapDvp,
    recipients: &[ShieldedAddress; TERMS_RECIPIENTS],
    tx_viewing_key: &ViewingKey,
    salt: [u8; 16],
) -> Result<Vec<MessageData>> {
    let plaintext = terms.to_bytes();
    recipients
        .iter()
        .zip(terms_recipients(terms))
        .zip(FIRST_TERMS_MESSAGE_SLOT..)
        .map(|((recipient, pubkey), slot)| {
            ensure!(
                recipient.solana_address().map_err(err)? == pubkey,
                "terms message recipient {slot} is not the party the terms name"
            );
            Ok(MessageData {
                view_tag: recipient.confidential_view_tag().map_err(err)?,
                data: tx_viewing_key
                    .encrypt_slot(&recipient.viewing_pubkey, &plaintext, salt, slot)
                    .map_err(err)?,
            })
        })
        .collect()
}

/// Decrypts message `message_index` of a create transaction with the
/// recipient's viewing key. CTR has no authentication, so a wrong key yields
/// garbage that either fails to parse or fails the caller's DvP UTXO check.
pub fn decrypt_terms_message(
    viewing_key: &ViewingKey,
    data: &[u8],
    tx_viewing_pk: &P256Pubkey,
    salt: [u8; 16],
    message_index: u32,
) -> Result<SwapDvp> {
    let slot = FIRST_TERMS_MESSAGE_SLOT
        .checked_add(message_index)
        .ok_or_else(|| err("terms message index overflows the slot"))?;
    let plaintext = viewing_key
        .decrypt_utxo(data, tx_viewing_pk, salt, slot)
        .map_err(err)?;
    SwapDvp::from_bytes(&plaintext)
}

//! The create transaction's terms messages (the plaintext `SwapDvp`
//! encrypted to the seller, the buyer and the settlement authority), and the
//! output ciphertexts of the transactions that spend DvP UTXOs.

use anyhow::{anyhow, ensure, Result};
use zolana_interface::instruction::instruction_data::transact::{
    MessageData, OwnerTag, TransactOutput,
};
use zolana_keypair::{random_salt, P256Pubkey, ShieldedAddress, ViewingKey};
use zolana_transaction::{
    serialization::confidential::{Confidential, ConfidentialEncode, ConfidentialOutputPlaintext},
    ExternalData, SppProofOutputUtxo, UtxoSerialization,
};

use crate::{err, state::SwapDvp};

/// Recipients of the terms messages, in message order.
pub const TERMS_RECIPIENTS: usize = 3;

/// Ciphertext slot of the first terms message. Messages continue the
/// slot numbering after the instruction's outputs; the create transaction
/// publishes one output (the DvP UTXO), its compact padding is omitted.
const FIRST_TERMS_MESSAGE_SLOT: u32 = 1;

/// The Solana pubkeys of the terms recipients, in message order: seller,
/// buyer, settlement authority.
pub(crate) fn terms_recipients(terms: &SwapDvp) -> [solana_address::Address; TERMS_RECIPIENTS] {
    [terms.seller, terms.buyer, terms.settlement_authority]
}

/// One message per recipient `j`, encrypted under the transaction viewing key
/// into slot `1 + j`. `data` is the bare AES-CTR ciphertext: a viewing
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

/// The external data of a transaction whose `outputs` carry their final
/// blindings: every output but compact padding is encrypted to its owner
/// address under `tx_viewing_key` and a fresh salt, in its slot, and tagged
/// with the owner's view tag. Compact padding is the trailing suffix SPP
/// fills back in, so it publishes nothing. The ciphertext is what a wallet
/// decrypts as a spendable note (the `ConfidentialTransaction::encrypt`
/// layout, without its ring and dummy-ciphertext cases).
pub(crate) fn encrypt_outputs(
    outputs: &[SppProofOutputUtxo],
    tx_viewing_key: &ViewingKey,
    output_tree_id: u16,
) -> Result<ExternalData> {
    let salt = random_salt();
    let mut transact_outputs = Vec::with_capacity(outputs.len());
    let mut view_tags = Vec::with_capacity(outputs.len());
    for (slot_index, output) in (0u32..).zip(outputs).filter(|(_, output)| !output.compact) {
        let owner = output
            .owner_address
            .ok_or_else(|| anyhow!("output {slot_index} has no owner address"))?;
        ensure!(
            output.ring_program_id.is_none(),
            "output {slot_index} is a ring output"
        );
        let view_tag = owner.confidential_view_tag().map_err(err)?;
        let message = Confidential::encode_plaintext(
            &ConfidentialOutputPlaintext {
                asset_id: output.asset.asset_id,
                amount: output.amount,
                blinding: output.blinding,
                ring_program_id: None,
                data: output.data.clone(),
            },
            view_tag,
            &ConfidentialEncode {
                tx: tx_viewing_key.clone(),
                recipient_pubkey: owner.viewing_pubkey,
                salt,
                slot_index,
            },
        )
        .map_err(err)?;
        transact_outputs.push(TransactOutput {
            utxo_hash: output.hash(output_tree_id).map_err(err)?,
            owner_tag: OwnerTag::Inline(view_tag),
            data: Some(message.data),
        });
        view_tags.push(view_tag);
    }
    Ok(ExternalData::new(
        *tx_viewing_key.pubkey().as_bytes(),
        salt,
        transact_outputs,
        view_tags,
        Vec::new(),
    ))
}

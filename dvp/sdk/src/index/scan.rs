use anyhow::Result;
use zolana_interface::{
    event::OutputDataEncoding, instruction::instruction_data::transact::MessageData,
};
use zolana_transaction::{
    instructions::transact::OutputSlot, AssetRegistry, EncryptedScheme, Mint, ShieldedTransaction,
    SOL_ASSET_ID,
};

use crate::err;

/// The messages of `tx` tagged `tag`, with their message index.
pub(crate) fn tagged_messages<'a>(
    tx: &'a ShieldedTransaction,
    tag: &'a [u8; 32],
) -> impl Iterator<Item = (usize, &'a MessageData)> {
    tx.messages
        .iter()
        .enumerate()
        .filter(move |(_, message)| &message.view_tag == tag)
}

/// The output slots of `tx` tagged `tag`.
pub(crate) fn tagged_outputs<'a>(
    tx: &'a ShieldedTransaction,
    tag: &'a [u8; 32],
) -> impl Iterator<Item = &'a OutputSlot> {
    tx.output_slots
        .iter()
        .filter(move |slot| &slot.view_tag == tag)
}

/// The output slots of `tx` tagged `tag` that carry a confidential
/// ciphertext, with the ciphertext body and the slot index it was encrypted
/// under. The index is the slot's position among all outputs, as the sender
/// encrypts it and as `zolana_transaction::decrypt` reads it. A slot without
/// data, such as the DvP UTXO, is skipped.
pub(crate) fn tagged_confidential_outputs<'a>(
    tx: &'a ShieldedTransaction,
    tag: &'a [u8; 32],
) -> impl Iterator<Item = (u32, &'a OutputSlot, Vec<u8>)> {
    (0u32..)
        .zip(tx.output_slots.iter())
        .filter(move |(_, slot)| &slot.view_tag == tag)
        .filter_map(|(slot_index, slot)| {
            let OutputDataEncoding::Encrypted(blob) = slot.output_data()? else {
                return None;
            };
            let (&scheme, body) = blob.split_first()?;
            (EncryptedScheme::from_byte(scheme).ok()? == EncryptedScheme::Confidential)
                .then(|| (slot_index, slot, body.to_vec()))
        })
}

/// The mint of a ciphertext's `asset_id`; SOL needs no registry entry.
pub(crate) fn resolve_mint(registry: &AssetRegistry, asset_id: u64) -> Result<Mint> {
    if asset_id == SOL_ASSET_ID {
        return Ok(Mint::SOL);
    }
    registry.resolve(asset_id).map_err(err)
}

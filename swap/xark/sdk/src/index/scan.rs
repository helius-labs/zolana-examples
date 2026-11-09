use anyhow::{anyhow, Result};
use spr_interface::event::OutputDataEncoding;
use spr_transaction::{
    instructions::transact::OutputSlot, AssetRegistry, DataRecord, EncryptedScheme,
    ShieldedTransaction, SOL_ASSET_ID,
};

use crate::{err, state::OrderTerms};

pub(crate) fn resolve_mint(
    registry: &AssetRegistry,
    asset_id: u64,
) -> Result<spr_transaction::Mint> {
    if asset_id == SOL_ASSET_ID {
        return Ok(spr_transaction::Mint::SOL);
    }
    registry.resolve(asset_id).map_err(err)
}

/// Unified confidential ciphertext slots with their decryption slot index:
/// ciphertext slots are indexed over data-bearing slots only.
pub(crate) fn unified_slots(
    tx: &ShieldedTransaction,
) -> impl Iterator<Item = (u32, &OutputSlot, Vec<u8>)> {
    let mut next_index = 0u32;
    tx.output_slots.iter().filter_map(move |slot| {
        let output_data = slot.output_data()?;
        let slot_index = next_index;
        next_index += 1;
        let OutputDataEncoding::Encrypted(mut blob) = output_data else {
            return None;
        };
        let scheme = EncryptedScheme::from_byte(*blob.first()?).ok()?;
        (scheme == EncryptedScheme::Confidential).then(|| {
            blob.drain(..1);
            (slot_index, slot, blob)
        })
    })
}

/// The order terms an order UTXO's decrypted `utxo_data` record carries.
pub(crate) fn parse_order_terms(
    records: &[DataRecord],
    registry: &AssetRegistry,
) -> Result<OrderTerms> {
    let payload = records
        .iter()
        .find_map(|record| match record {
            DataRecord::UtxoData(bytes) => Some(bytes.as_slice()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("order plaintext carries no utxo data record"))?;
    OrderTerms::from_payload(payload, registry)
}

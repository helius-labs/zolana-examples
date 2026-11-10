use anyhow::Result;
use solana_address::Address;
use spr_interface::event::OutputDataEncoding;
use spr_program::{DataUtxo, ProgramOwner};
use spr_transaction::{
    instructions::transact::OutputSlot, serialization::confidential::ConfidentialOutputPlaintext,
    AssetRegistry, EncryptedScheme, ShieldedTransaction, Utxo, WalletUtxo, SOL_ASSET_ID,
};
use swap_program::circuits;

use crate::{err, order_authority};

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

/// Decodes the decrypted output in `slot` of `tx`, of `asset`, as an order
/// UTXO of `maker`: owned by the maker's order authority, with order terms as
/// its state. The SDK's decode checks the state and the opening reproduce the
/// slot's committed hash, which commits the owner.
pub(crate) fn decode_order(
    tx: &ShieldedTransaction,
    slot: &OutputSlot,
    slot_index: u32,
    plaintext: ConfidentialOutputPlaintext,
    asset: spr_transaction::Mint,
    maker: &Address,
) -> Result<DataUtxo<circuits::OrderTerms>> {
    let utxo = WalletUtxo {
        utxo: Utxo {
            owner: order_authority(maker).public_key(),
            asset,
            amount: plaintext.amount,
            blinding: plaintext.blinding,
            ring_program_id: plaintext.ring_program_id,
            data: plaintext.data,
        },
        nullifier_pubkey: ProgramOwner::nullifier_pubkey().map_err(err)?,
        utxo_hash: slot.output_context.hash,
        nullifier: [0; 32],
        tx_viewing_key: None,
        data_hash: None,
        ring_data_hash: None,
        tree_id: slot.output_context.tree_id,
        leaf_index: slot.output_context.leaf_index,
        latest_tree_id: None,
        slot: tx.slot,
        tx_signature: tx.tx_signature,
        slot_index,
    };
    DataUtxo::try_from(utxo).map_err(err)
}

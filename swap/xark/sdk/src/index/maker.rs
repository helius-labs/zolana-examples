use std::time::Duration;

use anyhow::Result;
use spr_client::Rpc;
use spr_keypair::{ShieldedKeypair, X25519Pubkey};
use spr_transaction::{
    serialization::confidential::{Confidential, ConfidentialOutputPlaintext},
    AssetRegistry, ShieldedTransaction,
};

use super::{
    poll::{collect_tagged, index_until},
    scan::{parse_order_terms, resolve_mint, unified_slots},
};
use crate::{err, shared::INDEXED_TREE_ID, state::OrderUtxo};

/// An order rediscovered by its maker from its own make transaction.
#[derive(Debug)]
pub struct MakerOrder {
    pub order_utxo: OrderUtxo,
    pub taker_viewing_pubkey: X25519Pubkey,
}

/// Maker-side order rediscovery: the per-transaction viewing key re-derives
/// from the maker's viewing key and the first input's nullifier (a match against
/// `tx_viewing_pk` proves the maker authored the transaction). Each unified slot
/// embeds its recipient viewing pubkey, so that key decrypts every slot from
/// the sender side directly; the opening is accepted only if the terms name
/// this maker and the reconstructed order UTXO hash matches the slot's
/// committed leaf.
pub fn scan_maker(
    tx: &ShieldedTransaction,
    keypair: &ShieldedKeypair,
    registry: &AssetRegistry,
) -> Result<Option<MakerOrder>> {
    let (Some(tx_viewing_pk), Some(salt)) = (tx.tx_viewing_pk, tx.salt) else {
        return Ok(None);
    };
    let Some(tx_viewing_key) = tx.nullifiers.iter().find_map(|nullifier| {
        keypair
            .get_transaction_viewing_key(nullifier)
            .ok()
            .filter(|key| key.pubkey() == tx_viewing_pk)
    }) else {
        return Ok(None);
    };
    let maker_address = keypair.shielded_address().map_err(err)?;
    for (slot_index, slot, body) in unified_slots(tx) {
        let Ok(taker_viewing_pubkey) = Confidential::embedded_viewing_pk(&body) else {
            continue;
        };
        let Ok(plaintext) =
            Confidential::decrypt_with_tx_key(&tx_viewing_key, &body, salt, slot_index)
        else {
            continue;
        };
        let Some(order_utxo) = order_candidate(registry, plaintext) else {
            continue;
        };
        if order_utxo.terms.maker != maker_address {
            continue;
        }
        if order_utxo.hash(INDEXED_TREE_ID).ok() != Some(slot.output_context.hash) {
            continue;
        }
        return Ok(Some(MakerOrder {
            order_utxo,
            taker_viewing_pubkey,
        }));
    }
    Ok(None)
}

fn order_candidate(
    registry: &AssetRegistry,
    plaintext: ConfidentialOutputPlaintext,
) -> Option<OrderUtxo> {
    Some(OrderUtxo {
        terms: parse_order_terms(&plaintext.data.records, registry).ok()?,
        blinding: plaintext.blinding,
        source_mint: resolve_mint(registry, plaintext.asset_id).ok()?,
        source_amount: plaintext.amount,
    })
}

pub fn index_maker<I: Rpc>(
    keypair: &ShieldedKeypair,
    registry: &AssetRegistry,
    indexer: &I,
    timeout: Duration,
) -> Result<Vec<MakerOrder>> {
    index_until(timeout, "maker orders", || {
        collect_tagged(keypair, indexer, |tx| scan_maker(tx, keypair, registry))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::fixture::order_fixture;

    #[test]
    fn scan_maker_reconstructs_the_opening_from_the_makers_side() {
        let fixture = order_fixture();
        let order = scan_maker(&fixture.tx, &fixture.maker_keypair, &fixture.registry)
            .expect("scan")
            .expect("own order");
        assert_eq!(
            (order.order_utxo, order.taker_viewing_pubkey),
            (fixture.order_utxo, fixture.taker_keypair.viewing_pubkey())
        );
    }

    #[test]
    fn scan_maker_ignores_transactions_of_other_makers() {
        let fixture = order_fixture();
        assert!(
            scan_maker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
                .expect("scan")
                .is_none()
        );
    }
}

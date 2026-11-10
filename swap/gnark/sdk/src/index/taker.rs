use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use borsh::BorshDeserialize;
use solana_address::Address;
use solana_pubkey::Pubkey;
use spr_client::{user_registry::try_resolve_registered_address, Rpc};
use spr_keypair::{ShieldedAddress, ShieldedKeypair, X25519Pubkey};
use spr_transaction::{
    serialization::confidential::Confidential, utxo::Blinding, AssetRegistry, DecodeCx,
    ShieldedTransaction, UtxoSerialization,
};

use super::{
    poll::{collect_tagged, index_until},
    scan::{parse_order_data, resolve_mint, unified_slots},
};
use crate::{
    err,
    shared::INDEXED_TREE_ID,
    state::{OrderTerms, OrderUtxo, PlainTextData},
    MarkerData,
};

#[derive(Debug)]
pub struct TakerOrder {
    pub order_utxo: OrderUtxo,
    pub maker_pubkey: Pubkey,
}

pub struct TakerOrderCandidate {
    pub source_amount: u64,
    pub source_mint: spr_transaction::Mint,
    pub destination_mint: Address,
    pub order_utxo_blinding: Blinding,
    pub order_data: PlainTextData,
    pub maker_pubkey: Pubkey,
    pub order_utxo_hash: [u8; 32],
}

/// The order the marker tagged to `keypair`'s taker names in `tx`, or `None`
/// when `tx` has no such marker or does not decode as an order for it. Anyone
/// can post a message under the taker's view tag, so a malformed marker or
/// order output skips the transaction instead of failing the scan.
pub fn scan_taker(
    tx: &ShieldedTransaction,
    keypair: &ShieldedKeypair,
    registry: &AssetRegistry,
) -> Result<Option<TakerOrderCandidate>> {
    let taker_tag = keypair
        .signing_pubkey()
        .confidential_view_tag()
        .map_err(err)?;
    let Some(marker_message) = tx
        .messages
        .iter()
        .find(|message| message.view_tag == taker_tag)
    else {
        return Ok(None);
    };
    Ok(decode_candidate(tx, keypair, &marker_message.data, registry).ok())
}

/// Decodes the order `marker_data` names in `tx`.
fn decode_candidate(
    tx: &ShieldedTransaction,
    keypair: &ShieldedKeypair,
    marker_data: &[u8],
    registry: &AssetRegistry,
) -> Result<TakerOrderCandidate> {
    let marker =
        MarkerData::try_from_slice(marker_data).map_err(|e| anyhow!("marker payload: {e}"))?;
    let Some((slot_index, _, body)) =
        unified_slots(tx).find(|(_, slot, _)| slot.output_context.hash == marker.order_utxo_hash)
    else {
        bail!("marker without a unified order ciphertext in the same transaction");
    };
    let cx = DecodeCx::for_slot(&keypair.viewing_key, tx, slot_index);
    let order_utxo_plaintext = Confidential::decode(&body, &cx).map_err(err)?;
    let order_data = parse_order_data(&order_utxo_plaintext.data.records)?;
    Ok(TakerOrderCandidate {
        source_amount: order_utxo_plaintext.amount,
        source_mint: resolve_mint(registry, order_utxo_plaintext.asset_id)?,
        destination_mint: resolve_mint(registry, order_data.destination_asset_id)?.asset,
        order_utxo_blinding: order_utxo_plaintext.blinding,
        order_data,
        maker_pubkey: Pubkey::new_from_array(marker.maker_pubkey),
        order_utxo_hash: marker.order_utxo_hash,
    })
}

impl TakerOrderCandidate {
    pub fn into_order(
        self,
        destination: ShieldedAddress,
        taker_viewing_pubkey: X25519Pubkey,
    ) -> Result<TakerOrder> {
        let terms = OrderTerms {
            destination_mint: self.destination_mint,
            destination_amount: self.order_data.destination_amount,
            destination,
            taker: self.order_data.taker,
            expiry: self.order_data.expiry,
        };
        let order_utxo = OrderUtxo {
            terms,
            blinding: self.order_utxo_blinding,
            source_mint: self.source_mint,
            source_amount: self.source_amount,
            destination_asset_id: self.order_data.destination_asset_id,
        };
        let order_utxo_hash = order_utxo
            .output_utxo(taker_viewing_pubkey)?
            .hash(INDEXED_TREE_ID)
            .map_err(err)?;
        if order_utxo_hash != self.order_utxo_hash {
            bail!("reconstructed order utxo hash does not match the committed leaf");
        }
        Ok(TakerOrder {
            order_utxo,
            maker_pubkey: self.maker_pubkey,
        })
    }
}

pub fn index_taker<I: Rpc, R: Rpc>(
    keypair: &ShieldedKeypair,
    registry: &AssetRegistry,
    indexer: &I,
    rpc: &R,
    timeout: Duration,
) -> Result<Vec<TakerOrder>> {
    let taker_viewing_pubkey = keypair.viewing_pubkey();
    index_until(timeout, "taker orders", || {
        collect_tagged(keypair, indexer, |tx| {
            let Some(candidate) = scan_taker(tx, keypair, registry)? else {
                return Ok(None);
            };
            // RPC errors propagate. A maker missing from the registry or an
            // order that names another maker skips the transaction.
            let Some(maker_resolved_address) =
                try_resolve_registered_address(rpc, candidate.maker_pubkey).map_err(err)?
            else {
                return Ok(None);
            };
            Ok(candidate
                .into_order(maker_resolved_address.address, taker_viewing_pubkey)
                .ok())
        })
    })
}

#[cfg(test)]
mod tests {
    use solana_keypair::Keypair;

    use super::*;
    use crate::index::fixture::{order_fixture, OrderFixture};

    #[test]
    fn scan_taker_reconstructs_terms_from_the_transaction() {
        let fixture = order_fixture();
        let candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        let order = candidate
            .into_order(
                fixture.maker_address,
                fixture.taker_keypair.viewing_pubkey(),
            )
            .expect("order");
        assert_eq!(
            (order.order_utxo, order.maker_pubkey),
            (fixture.order_utxo, fixture.maker_pubkey)
        );
    }

    #[test]
    fn into_order_rejects_a_wrong_maker_address() {
        let fixture = order_fixture();
        let candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        let taker_address = fixture
            .taker_keypair
            .shielded_address()
            .expect("taker address");
        let error = candidate
            .into_order(taker_address, fixture.taker_keypair.viewing_pubkey())
            .expect_err("wrong maker address must fail the hash check");
        assert!(error
            .to_string()
            .contains("does not match the committed leaf"));
    }

    /// The fixture with the data of the taker's marker message replaced.
    fn fixture_with_marker(data: impl FnOnce(&[u8]) -> Vec<u8>) -> OrderFixture {
        let mut fixture = order_fixture();
        let taker_tag = fixture
            .taker_keypair
            .signing_pubkey()
            .confidential_view_tag()
            .expect("taker tag");
        let marker = fixture
            .tx
            .messages
            .iter_mut()
            .find(|message| message.view_tag == taker_tag)
            .expect("taker marker");
        marker.data = data(&marker.data);
        fixture
    }

    #[test]
    fn scan_taker_skips_a_malformed_marker() {
        let fixture = fixture_with_marker(|data| data.iter().take(10).copied().collect());
        assert!(
            scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
                .expect("a malformed marker must not fail the scan")
                .is_none()
        );
    }

    #[test]
    fn scan_taker_skips_a_marker_without_its_order_output() {
        let fixture = fixture_with_marker(|data| {
            let mut marker = MarkerData::try_from_slice(data).expect("marker");
            marker.order_utxo_hash = [1u8; 32];
            borsh::to_vec(&marker).expect("marker bytes")
        });
        assert!(
            scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
                .expect("a marker naming no order output must not fail the scan")
                .is_none()
        );
    }

    #[test]
    fn scan_taker_skips_an_order_it_cannot_decrypt() {
        // A marker tagged to another key over an order encrypted to the taker.
        let fixture = order_fixture();
        let other_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([21u8; 32]))
            .expect("other keypair");
        let mut tx = fixture.tx;
        let other_tag = other_keypair
            .signing_pubkey()
            .confidential_view_tag()
            .expect("other tag");
        for message in tx.messages.iter_mut() {
            message.view_tag = other_tag;
        }
        assert!(scan_taker(&tx, &other_keypair, &fixture.registry)
            .expect("an undecryptable order must not fail the scan")
            .is_none());
    }

    #[test]
    fn scan_taker_ignores_transactions_for_other_takers() {
        let fixture = order_fixture();
        let other_keypair = ShieldedKeypair::from_keypair(&Keypair::new_from_array([21u8; 32]))
            .expect("other keypair");
        assert!(scan_taker(&fixture.tx, &other_keypair, &fixture.registry)
            .expect("scan")
            .is_none());
    }
}

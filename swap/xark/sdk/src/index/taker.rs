use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use borsh::BorshDeserialize;
use solana_address::Address;
use spr_client::{user_registry::resolve_registered_address, Rpc};
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_transaction::{
    serialization::confidential::Confidential, AssetRegistry, DecodeCx, ShieldedTransaction,
    UtxoSerialization,
};

use super::{
    poll::{collect_tagged, index_until},
    scan::{parse_order_terms, resolve_mint, unified_slots},
};
use crate::{err, shared::INDEXED_TREE_ID, state::OrderUtxo, MarkerData};

#[derive(Debug)]
pub struct TakerOrder {
    pub order_utxo: OrderUtxo,
    pub maker_pubkey: Address,
}

/// An order decrypted from a make transaction, before the maker it names is
/// checked against the user registry.
pub struct TakerOrderCandidate {
    pub order_utxo: OrderUtxo,
    pub maker_pubkey: Address,
    pub order_utxo_hash: [u8; 32],
}

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
    let marker = MarkerData::try_from_slice(&marker_message.data)
        .map_err(|e| anyhow!("marker payload: {e}"))?;
    let Some((slot_index, _, body)) =
        unified_slots(tx).find(|(_, slot, _)| slot.output_context.hash == marker.order_utxo_hash)
    else {
        bail!("marker without a unified order ciphertext in the same transaction");
    };
    let cx = DecodeCx::for_slot(&keypair.viewing_key, tx, slot_index);
    let plaintext = Confidential::decode(&body, &cx).map_err(err)?;
    Ok(Some(TakerOrderCandidate {
        order_utxo: OrderUtxo {
            terms: parse_order_terms(&plaintext.data.records, registry)?,
            blinding: plaintext.blinding,
            source_mint: resolve_mint(registry, plaintext.asset_id)?,
            source_amount: plaintext.amount,
        },
        maker_pubkey: Address::new_from_array(marker.maker_pubkey),
        order_utxo_hash: marker.order_utxo_hash,
    }))
}

impl TakerOrderCandidate {
    /// Accepts the order if the maker it names is `registered_maker`, the
    /// address the user registry holds for the marker's maker pubkey, and the
    /// decrypted opening recomputes the order UTXO hash the marker names.
    pub fn into_order(self, registered_maker: ShieldedAddress) -> Result<TakerOrder> {
        if self.order_utxo.terms.maker != registered_maker {
            bail!("the order names a maker other than the registered marker signer");
        }
        if self.order_utxo.hash(INDEXED_TREE_ID)? != self.order_utxo_hash {
            bail!("reconstructed order utxo hash does not match the committed leaf");
        }
        Ok(TakerOrder {
            order_utxo: self.order_utxo,
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
    index_until(timeout, "taker orders", || {
        collect_tagged(keypair, indexer, |tx| {
            let Some(candidate) = scan_taker(tx, keypair, registry)? else {
                return Ok(None);
            };
            let maker = resolve_registered_address(rpc, candidate.maker_pubkey).map_err(err)?;
            candidate.into_order(maker.address).map(Some)
        })
    })
}

#[cfg(test)]
mod tests {
    use solana_keypair::Keypair;

    use super::*;
    use crate::index::fixture::order_fixture;

    #[test]
    fn scan_taker_reconstructs_terms_from_the_transaction() {
        let fixture = order_fixture();
        let candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        let order = candidate.into_order(fixture.maker_address).expect("order");
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
            .into_order(taker_address)
            .expect_err("a maker other than the registered one must fail");
        assert!(error.to_string().contains("other than the registered"));
    }

    #[test]
    fn into_order_rejects_a_wrong_order_utxo_hash() {
        let fixture = order_fixture();
        let mut candidate = scan_taker(&fixture.tx, &fixture.taker_keypair, &fixture.registry)
            .expect("scan")
            .expect("order candidate");
        candidate.order_utxo.source_amount += 1;
        let error = candidate
            .into_order(fixture.maker_address)
            .expect_err("another opening must fail the hash check");
        assert!(error
            .to_string()
            .contains("does not match the committed leaf"));
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

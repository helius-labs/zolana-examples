use std::time::Duration;

use solana_address::Address;
use solana_signature::Signature;
use zolana_client::Rpc;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::ShieldedTransaction;

use anyhow::Result;

use super::{
    poll::{collect_tagged, index_until},
    scan::{tagged_messages, tagged_outputs},
};
use crate::{
    encryption::{decrypt_terms_message, terms_recipients, TERMS_RECIPIENTS},
    err,
    state::{DvpUtxo, SwapDvp},
};

/// A DvP a party found in a create transaction: terms it decrypted from its
/// message and the DvP UTXO they recompute, matched against the committed
/// leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredDvp {
    pub dvp_utxo: DvpUtxo,
    pub leaf_index: u64,
    /// The message the terms came from: 0 seller, 1 buyer, 2 settlement
    /// authority.
    pub recipient_index: u32,
    pub tx_signature: Signature,
}

/// Finds the terms message for `keypair` in `tx`:
///
/// 1. a message among the first `TERMS_RECIPIENTS`, tagged with the
///    keypair's pubkey;
/// 2. that decrypts, under the keypair's viewing key, into terms naming the
///    keypair as recipient of that message;
/// 3. whose DvP UTXO, recomputed with the tree id of an output tagged with
///    the terms' DvP authority PDA, equals that output's committed hash.
///
/// The hash check is the only integrity check: the AES-CTR message is not
/// authenticated (D7). It also proves the terms hash to the output's
/// `dvp_id`, since the DvP UTXO's data hash is `dvp_id`.
pub fn scan_terms(
    tx: &ShieldedTransaction,
    keypair: &ShieldedKeypair,
) -> Result<Option<DiscoveredDvp>> {
    let (Some(tx_viewing_pk), Some(salt)) = (tx.tx_viewing_pk, tx.salt) else {
        return Ok(None);
    };
    let own_tag = keypair
        .signing_pubkey()
        .confidential_view_tag()
        .map_err(err)?;
    let own_pubkey = Address::new_from_array(own_tag);
    for (index, message) in
        tagged_messages(tx, &own_tag).take_while(|(index, _)| *index < TERMS_RECIPIENTS)
    {
        let index = u32::try_from(index).map_err(err)?;
        let Ok(terms) = decrypt_terms_message(
            &keypair.viewing_key,
            &message.data,
            &tx_viewing_pk,
            salt,
            index,
        ) else {
            continue;
        };
        if terms_recipients(&terms).get(index as usize) != Some(&own_pubkey) {
            continue;
        }
        if let Some((dvp_utxo, leaf_index)) = committed_dvp_utxo(tx, terms) {
            return Ok(Some(DiscoveredDvp {
                dvp_utxo,
                leaf_index,
                recipient_index: index,
                tx_signature: tx.tx_signature,
            }));
        }
    }
    Ok(None)
}

/// The DvP UTXO of `terms` and its leaf index, if `tx` commits it in an
/// output tagged with the terms' PDA.
fn committed_dvp_utxo(tx: &ShieldedTransaction, terms: SwapDvp) -> Option<(DvpUtxo, u64)> {
    let (pda, _) = terms.dvp_authority().ok()?;
    let pda_tag = pda.to_bytes();
    // `DvpUtxo::hash` validates the terms first (`SwapDvp::validate`), so
    // terms the create circuit accepts but the sdk refuses (D10) are not
    // discovered. The binding ends the iterator's borrow of `pda_tag`.
    let committed = tagged_outputs(tx, &pda_tag).find_map(|slot| {
        let context = &slot.output_context;
        let dvp_utxo = DvpUtxo::new(terms.clone(), context.tree_id);
        (dvp_utxo.hash().ok()? == context.hash).then_some((dvp_utxo, context.leaf_index))
    });
    committed
}

/// Polls the indexer until the DvPs it serves with a terms message for
/// `keypair` ([`scan_terms`]), found by the keypair's own tag, satisfy
/// `ready` (for example: one with a given `dvp_id`). A party with older DvPs
/// waits for the one it expects instead of returning the old ones.
pub fn index_terms<I: Rpc>(
    keypair: &ShieldedKeypair,
    indexer: &I,
    timeout: Duration,
    ready: impl Fn(&[DiscoveredDvp]) -> bool,
) -> Result<Vec<DiscoveredDvp>> {
    let own_tag = keypair
        .signing_pubkey()
        .confidential_view_tag()
        .map_err(err)?;
    index_until(
        timeout,
        "DvP terms",
        || collect_tagged(own_tag, indexer, |tx| scan_terms(tx, keypair)),
        ready,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::fixture::{keypair, terms_fixture, FIXTURE_LEAF_INDEX};

    #[test]
    fn every_party_discovers_the_terms_and_the_dvp_utxo() {
        let fixture = terms_fixture();
        for (recipient_index, party) in (0u32..).zip(fixture.parties.iter()) {
            let found = scan_terms(&fixture.tx, party)
                .expect("scan")
                .expect("terms");
            assert_eq!(
                found,
                DiscoveredDvp {
                    dvp_utxo: fixture.dvp_utxo.clone(),
                    leaf_index: FIXTURE_LEAF_INDEX,
                    recipient_index,
                    tx_signature: fixture.tx.tx_signature,
                }
            );
        }
    }

    #[test]
    fn an_outsider_finds_nothing() {
        let fixture = terms_fixture();
        let outsider = keypair(99);
        assert!(scan_terms(&fixture.tx, &outsider).expect("scan").is_none());
    }

    #[test]
    fn a_wrong_committed_hash_is_rejected() {
        let mut fixture = terms_fixture();
        for slot in &mut fixture.tx.output_slots {
            slot.output_context.hash = [1; 32];
        }
        for party in &fixture.parties {
            assert!(scan_terms(&fixture.tx, party).expect("scan").is_none());
        }
    }

    #[test]
    fn an_output_not_tagged_with_the_pda_is_ignored() {
        let mut fixture = terms_fixture();
        for slot in &mut fixture.tx.output_slots {
            slot.view_tag = [2; 32];
        }
        for party in &fixture.parties {
            assert!(scan_terms(&fixture.tx, party).expect("scan").is_none());
        }
    }

    #[test]
    fn a_message_moved_to_another_slot_is_rejected() {
        let mut fixture = terms_fixture();
        // The seller's message re-tagged into the buyer's position decrypts
        // under the wrong slot, and its terms name the seller, not the buyer.
        fixture.tx.messages.swap(0, 1);
        let [seller, buyer, _] = &fixture.parties;
        assert!(scan_terms(&fixture.tx, seller).expect("scan").is_none());
        assert!(scan_terms(&fixture.tx, buyer).expect("scan").is_none());
    }
}

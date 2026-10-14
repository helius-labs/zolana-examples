use std::time::Duration;

use anyhow::Result;
use zolana_client::Rpc;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{
    serialization::confidential::{Confidential, ConfidentialOutputPlaintext},
    AssetRegistry, DecodeCx, ShieldedTransaction, UtxoSerialization,
};

use super::{
    poll::{collect_tagged, index_until},
    scan::{resolve_mint, tagged_confidential_outputs},
};
use crate::state::{Deposit, Leg, SwapDvp};

/// Settlement-authority deposit discovery: every output of `tx` tagged with
/// the terms' DvP authority PDA whose ciphertext decrypts under the
/// authority's viewing key into a deposit of one of the legs
/// ([`committed_deposits`]). The DvP UTXO shares the tag but carries no
/// ciphertext (D9), so it is skipped.
pub fn scan_deposits(
    tx: &ShieldedTransaction,
    terms: &SwapDvp,
    authority: &ShieldedKeypair,
    registry: &AssetRegistry,
) -> Result<Vec<Deposit>> {
    committed_deposits(tx, terms, registry, |slot_index, body| {
        let cx = DecodeCx::for_slot(&authority.viewing_key, tx, slot_index);
        Confidential::decode(body, &cx).ok()
    })
}

/// Funder-side deposit discovery, without the authority's viewing key: the
/// transaction viewing key re-derives from the funder's viewing key and one
/// of `tx`'s nullifiers (a match against `tx_viewing_pk` proves the funder
/// built the transaction), and it decrypts every slot from the sender side.
pub fn scan_own_deposits(
    tx: &ShieldedTransaction,
    terms: &SwapDvp,
    funder: &ShieldedKeypair,
    registry: &AssetRegistry,
) -> Result<Vec<Deposit>> {
    let (Some(tx_viewing_pk), Some(salt)) = (tx.tx_viewing_pk, tx.salt) else {
        return Ok(Vec::new());
    };
    let Some(tx_viewing_key) = tx.nullifiers.iter().find_map(|nullifier| {
        funder
            .get_transaction_viewing_key(nullifier)
            .ok()
            .filter(|key| key.pubkey() == tx_viewing_pk)
    }) else {
        return Ok(Vec::new());
    };
    committed_deposits(tx, terms, registry, |slot_index, body| {
        Confidential::decrypt_with_tx_key(&tx_viewing_key, body, salt, slot_index).ok()
    })
}

/// The deposits the PDA-tagged ciphertext slots of `tx` open under
/// `decrypt`: those whose asset is one of the legs and whose PDA-owned,
/// data-free UTXO hashes to the slot's committed leaf. The hash check is
/// what makes an opening spendable: a slot with data, another owner or a
/// wrong amount fails it.
fn committed_deposits(
    tx: &ShieldedTransaction,
    terms: &SwapDvp,
    registry: &AssetRegistry,
    decrypt: impl Fn(u32, &[u8]) -> Option<ConfidentialOutputPlaintext>,
) -> Result<Vec<Deposit>> {
    let pda_tag = terms.dvp_authority()?.0.to_bytes();
    Ok(tagged_confidential_outputs(tx, &pda_tag)
        .filter_map(|(slot_index, slot, body)| {
            let plaintext = decrypt(slot_index, &body)?;
            let mint = resolve_mint(registry, plaintext.asset_id).ok()?;
            let context = &slot.output_context;
            let deposit = Deposit {
                terms: terms.clone(),
                leg: Leg::of_mint(terms, &mint.asset)?,
                mint,
                amount: plaintext.amount,
                blinding: plaintext.blinding,
                tree_id: context.tree_id,
                leaf_index: context.leaf_index,
            };
            (deposit.hash().ok()? == context.hash).then_some(deposit)
        })
        .collect())
}

/// Polls the indexer until it serves deposits of `terms` the settlement
/// authority can read ([`scan_deposits`]), found by the PDA tag.
pub fn index_deposits<I: Rpc>(
    terms: &SwapDvp,
    authority: &ShieldedKeypair,
    registry: &AssetRegistry,
    indexer: &I,
    timeout: Duration,
) -> Result<Vec<Deposit>> {
    index_tagged(terms, indexer, timeout, "DvP deposits", |tx| {
        scan_deposits(tx, terms, authority, registry)
    })
}

/// Polls the indexer until it serves deposits of `terms` that `funder`
/// made ([`scan_own_deposits`]), found by the PDA tag.
pub fn index_own_deposits<I: Rpc>(
    terms: &SwapDvp,
    funder: &ShieldedKeypair,
    registry: &AssetRegistry,
    indexer: &I,
    timeout: Duration,
) -> Result<Vec<Deposit>> {
    index_tagged(terms, indexer, timeout, "own DvP deposits", |tx| {
        scan_own_deposits(tx, terms, funder, registry)
    })
}

fn index_tagged<I: Rpc>(
    terms: &SwapDvp,
    indexer: &I,
    timeout: Duration,
    what: &str,
    mut scan: impl FnMut(&ShieldedTransaction) -> Result<Vec<Deposit>>,
) -> Result<Vec<Deposit>> {
    let pda_tag = terms.dvp_authority()?.0.to_bytes();
    index_until(timeout, what, || {
        collect_tagged(pda_tag, indexer, &mut scan)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::fixture::{deposit_fixture, keypair, terms_fixture};

    #[test]
    fn the_settlement_authority_finds_both_legs() {
        let fixture = deposit_fixture();
        let found: Vec<Deposit> = fixture
            .txs
            .iter()
            .flat_map(|tx| {
                scan_deposits(tx, &fixture.terms, &fixture.authority, &fixture.registry)
                    .expect("scan")
            })
            .collect();
        assert_eq!(found, fixture.deposits.to_vec());
    }

    #[test]
    fn each_funder_finds_only_its_own_deposit() {
        let fixture = deposit_fixture();
        for (funder, expected) in fixture.funders.iter().zip(fixture.deposits.iter()) {
            let found: Vec<Deposit> = fixture
                .txs
                .iter()
                .flat_map(|tx| {
                    scan_own_deposits(tx, &fixture.terms, funder, &fixture.registry).expect("scan")
                })
                .collect();
            assert_eq!(found, vec![expected.clone()]);
        }
    }

    #[test]
    fn an_outsider_finds_nothing() {
        let fixture = deposit_fixture();
        let outsider = keypair(99);
        for tx in &fixture.txs {
            assert_eq!(
                (
                    scan_deposits(tx, &fixture.terms, &outsider, &fixture.registry).expect("scan"),
                    scan_own_deposits(tx, &fixture.terms, &outsider, &fixture.registry)
                        .expect("scan"),
                ),
                (Vec::new(), Vec::new())
            );
        }
    }

    #[test]
    fn a_wrong_committed_hash_is_rejected() {
        let mut fixture = deposit_fixture();
        for slot in fixture
            .txs
            .iter_mut()
            .flat_map(|tx| tx.output_slots.iter_mut())
        {
            slot.output_context.hash = [1; 32];
        }
        for tx in &fixture.txs {
            assert!(
                scan_deposits(tx, &fixture.terms, &fixture.authority, &fixture.registry)
                    .expect("scan")
                    .is_empty()
            );
        }
    }

    #[test]
    fn deposits_of_other_terms_are_ignored() {
        let fixture = deposit_fixture();
        let other_terms = SwapDvp {
            nonce: fixture.terms.nonce + 1,
            ..fixture.terms.clone()
        };
        for tx in &fixture.txs {
            assert!(
                scan_deposits(tx, &other_terms, &fixture.authority, &fixture.registry)
                    .expect("scan")
                    .is_empty()
            );
        }
    }

    #[test]
    fn the_dvp_utxo_is_not_a_deposit() {
        let fixture = terms_fixture();
        let [_, _, authority] = &fixture.parties;
        let found = scan_deposits(
            &fixture.tx,
            &fixture.dvp_utxo.terms,
            authority,
            &AssetRegistry::default(),
        )
        .expect("scan");
        assert!(found.is_empty());
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use rust_client_example::{asset_registry, cli_keypair, setup, SetupContext};
use solana_signature::Signature;
use zolana_client::{EncryptedUtxoMatch, Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::{P256Pubkey, ShieldedKeypair};
use zolana_transaction::{decrypt, Address, ShieldedKeys, ShieldedTransaction};

const PAGE_LIMIT: u32 = 1_000;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;

    // Resolve the SPL asset registrations so every UTXO maps to its mint.
    let assets = asset_registry(&client)?;

    // 1. Fetch the transactions tagged for the wallet: transfers carry the
    // owner tag, deposits the viewing key's tag.
    let mut tags = vec![sender.shielded_address()?.confidential_view_tag()?];
    tags.extend(sender.viewing_public_keys().iter().map(P256Pubkey::x));
    let mut seen = HashSet::new();
    let mut transactions = Vec::new();
    let mut batch = tagged_transactions(&client, &tags)?;

    // 2. Decrypt them, then fetch the transactions that spent the wallet's
    // UTXOs, until a round finds nothing new.
    let mut queried = HashSet::new();
    let decrypted = loop {
        batch.retain(|tx| seen.insert(transaction_key(tx)));
        transactions.append(&mut batch);
        let decrypted = decrypt(&sender, &transactions, &assets)?;
        let nullifiers: Vec<_> = decrypted
            .utxos
            .iter()
            .map(|utxo| utxo.nullifier)
            .filter(|nullifier| queried.insert(*nullifier))
            .collect();
        batch = spending_transactions(&client, &nullifiers)?;
        batch.retain(|tx| !seen.contains(&transaction_key(tx)));
        if batch.is_empty() {
            break decrypted;
        }
    };

    // 3. Net each transaction per mint: UTXOs it created for the wallet
    // minus UTXOs of the wallet it spent.
    let spent_by: HashMap<[u8; 32], (Address, u64)> = decrypted
        .utxos
        .iter()
        .map(|utxo| (utxo.nullifier, (utxo.utxo.asset.asset, utxo.utxo.amount)))
        .collect();
    transactions.sort_by_key(|tx| tx.slot);
    let mut printed = HashSet::new();
    for tx in &transactions {
        if !printed.insert(tx.tx_signature) {
            continue;
        }
        let mut received: BTreeMap<Address, u64> = BTreeMap::new();
        for utxo in decrypted
            .utxos
            .iter()
            .filter(|utxo| utxo.tx_signature == tx.tx_signature)
        {
            *received.entry(utxo.utxo.asset.asset).or_default() += utxo.utxo.amount;
        }
        let mut spent: BTreeMap<Address, u64> = BTreeMap::new();
        for (mint, amount) in tx.nullifiers.iter().filter_map(|n| spent_by.get(n)) {
            *spent.entry(*mint).or_default() += amount;
        }
        let mints: Vec<_> = received.keys().chain(spent.keys()).copied().collect();
        for mint in mints.into_iter().collect::<BTreeSet<_>>() {
            let (received, spent) = (
                received.get(&mint).copied().unwrap_or(0),
                spent.get(&mint).copied().unwrap_or(0),
            );
            let (kind, direction, amount) = if spent == 0 {
                let kind = if tx.proofless {
                    "deposit"
                } else {
                    "privateTransfer"
                };
                (kind, "inbound", received)
            } else if received >= spent {
                let kind = if tx.may_be_merge() {
                    "merge"
                } else {
                    "privateTransfer"
                };
                (kind, "self", spent)
            } else {
                ("spend", "outbound", spent - received)
            };
            if amount == 0 {
                continue;
            }
            println!(
                "ok kind={kind} direction={direction} mint={mint} amount={amount} tx={}",
                tx.tx_signature,
            );
        }
    }
    Ok(())
}

/// A proofless deposit arrives one output at a time, so its leaf is part of
/// its identity.
fn transaction_key(tx: &ShieldedTransaction) -> (Signature, Option<u16>, Option<u64>) {
    let leaf = tx
        .proofless
        .then(|| tx.output_slots.first())
        .flatten()
        .map(|slot| slot.output_context.leaf_index);
    (tx.tx_signature, tx.event_index, leaf)
}

/// Transfers tagged with any of `tags`, then deposits from the
/// encrypted-output stream, one transaction per output.
fn tagged_transactions<R: Rpc>(rpc: &R, tags: &[[u8; 32]]) -> Result<Vec<ShieldedTransaction>> {
    let mut transactions = Vec::new();
    let mut cursor = None;
    loop {
        let page =
            rpc.get_shielded_transactions_by_tags(tags.to_vec(), cursor, Some(PAGE_LIMIT), None)?;
        transactions.extend(page.transactions.into_iter().filter(|tx| !tx.proofless));
        let Some(next) = page.next_cursor else { break };
        cursor = Some(next);
    }
    let mut cursor = None;
    loop {
        let page =
            rpc.get_encrypted_utxos_by_tags(tags.to_vec(), cursor, Some(PAGE_LIMIT), None)?;
        transactions.extend(
            page.matches
                .into_iter()
                .filter_map(EncryptedUtxoMatch::into_proofless_transaction),
        );
        let Some(next) = page.next_cursor else { break };
        cursor = Some(next);
    }
    Ok(transactions)
}

/// The transactions that spent any of `nullifiers`.
fn spending_transactions<R: Rpc>(
    rpc: &R,
    nullifiers: &[[u8; 32]],
) -> Result<Vec<ShieldedTransaction>> {
    let mut transactions = Vec::new();
    for chunk in nullifiers.chunks(PAGE_LIMIT as usize) {
        let mut cursor = None;
        loop {
            let page = rpc.get_shielded_transactions_by_nullifiers(
                chunk.to_vec(),
                cursor,
                Some(PAGE_LIMIT),
                None,
            )?;
            transactions.extend(page.transactions);
            let Some(next) = page.next_cursor else { break };
            cursor = Some(next);
        }
    }
    Ok(transactions)
}

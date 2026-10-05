use std::collections::HashMap;

use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{decrypt, AssetRegistry, ShieldedTransaction, WalletUtxo};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // The Solana signer and the private wallet are derived from the same
    // Ed25519 seed.
    let owner = ShieldedKeypair::from_keypair(&cli_keypair()?)?;
    let assets = AssetRegistry::default();

    // Every confidential transaction to or from this wallet carries its owner
    // tag: payments to it, its own sends and their change.
    let owner_tag = owner
        .shielded_address()?
        .signing_pubkey
        .confidential_view_tag()?;
    let mut transactions: Vec<ShieldedTransaction> = Vec::new();
    let mut cursor = None;
    loop {
        let page =
            client.get_shielded_transactions_by_tags(vec![owner_tag], cursor, Some(50), None)?;
        transactions.extend(page.transactions);
        let Some(next) = page.next_cursor else { break };
        cursor = Some(next);
    }

    // Decrypt the notes these transactions created for this wallet, spent or
    // not. A transaction spends a note by publishing its nullifier.
    let decrypted = decrypt(&owner, &transactions, &assets)?;
    let by_nullifier: HashMap<[u8; 32], &WalletUtxo> = decrypted
        .utxos
        .iter()
        .map(|utxo| (utxo.nullifier, utxo))
        .collect();

    for tx in &transactions {
        let received = decrypted
            .utxos
            .iter()
            .filter(|utxo| utxo.tx_signature == tx.tx_signature);
        let spent = tx
            .nullifiers
            .iter()
            .filter_map(|nullifier| by_nullifier.get(nullifier).copied());
        println!(
            "ok tx={} slot={} received=[{}] spent=[{}]",
            tx.tx_signature,
            tx.slot,
            amounts(received),
            amounts(spent),
        );
    }
    Ok(())
}

/// `mint:amount` per note.
fn amounts<'a>(utxos: impl Iterator<Item = &'a WalletUtxo>) -> String {
    utxos
        .map(|utxo| format!("{}:{}", utxo.utxo.asset.asset, utxo.utxo.amount))
        .collect::<Vec<_>>()
        .join(",")
}

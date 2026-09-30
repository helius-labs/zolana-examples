use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{decrypt_spendable, AssetRegistry, ShieldedTransaction};

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

    // Decrypt locally. A note is spent when a fetched transaction spends it.
    for balance in decrypt_spendable(&owner, &transactions, &assets)?
        .balances
        .assets
    {
        println!(
            "ok solana_address={} mint={} amount={} utxos={}",
            owner.pubkey(),
            balance.mint,
            balance.amount,
            balance.utxos.len(),
        );
    }
    Ok(())
}

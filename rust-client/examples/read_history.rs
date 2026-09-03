use anyhow::{anyhow, Result};
use rust_client_example::{cli_keypair, setup, SetupContext};
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet, DEFAULT_TAG_WINDOW};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        prover_url,
        tree,
    );

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;
    let assets = AssetRegistry::default();

    // Fetch transaction outputs from the indexer.
    let response = client.get_shielded_transactions_by_tags(
        vec![sender.shielded_address()?.confidential_view_tag()?],
        None,
        Some(50),
        None,
    )?;

    // Decrypt locally to read the private history.
    let mut wallet = Wallet::new(sender.shielded_address()?, assets)
        .map_err(|e| anyhow!("create wallet: {e:?}"))?;
    wallet
        .sync(&sender, &response.transactions, 0, DEFAULT_TAG_WINDOW)
        .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;

    for tx in wallet.private_transactions() {
        println!(
            "ok kind={:?} direction={:?} mint={} amount={} tx={}",
            tx.kind, tx.direction, tx.asset, tx.amount, tx.id.signature,
        );
    }
    Ok(())
}

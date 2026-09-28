use anyhow::{anyhow, Result};
use rust_client_example::cli_keypair;
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::AssetRegistry;
use zolana_wallet::{sync_wallet_with_config, SyncWalletConfig, Wallet};

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let api_key = std::env::var("API_KEY")?;
    let url = format!("https://devnet.helius-rpc.com/?api-key={api_key}");
    let client = ZolanaClient::from_urls(SolanaRpc::new(&url), &url, &url)?;
    // localnet: zolana dev start. RPC port :8899, indexer port :8784, prover port :3001.
    // let client = ZolanaClient::from_urls(
    //     SolanaRpc::new("http://127.0.0.1:8899"),
    //     "http://127.0.0.1:8784",
    //     "http://127.0.0.1:3001",
    // )?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;
    let assets = AssetRegistry::default();

    // Sync all transaction pages and resolve SPL asset registrations.
    let mut wallet = Wallet::new(sender.shielded_address()?, assets)
        .map_err(|e| anyhow!("create wallet: {e:?}"))?;
    let report = sync_wallet_with_config(
        &mut wallet,
        &sender,
        &client,
        SyncWalletConfig {
            page_limit: 50,
            ..SyncWalletConfig::default()
        },
    )?;
    anyhow::ensure!(
        report.unknown_asset_ids.is_empty(),
        "could not resolve SPL asset registrations"
    );

    for tx in wallet.private_transactions() {
        println!(
            "ok kind={:?} direction={:?} mint={} amount={} tx={}",
            tx.kind, tx.direction, tx.asset, tx.amount, tx.id.signature,
        );
    }
    Ok(())
}

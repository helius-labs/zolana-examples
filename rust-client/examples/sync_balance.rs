use anyhow::{anyhow, Result};
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet};
use zolana_wallet::{sync_wallet_with_config, SyncWalletConfig};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url, tree)?;

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

    for b in wallet.balances(false)? {
        println!(
            "ok solana_address={} mint={} amount={} utxos={}",
            sender.pubkey(),
            b.mint,
            b.amount,
            b.utxos.len(),
        );
    }
    Ok(())
}

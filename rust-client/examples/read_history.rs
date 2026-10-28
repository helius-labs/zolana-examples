use anyhow::{anyhow, Result};
use rust_client_example::{asset_registry, cli_keypair, setup};
use solana_keypair::Keypair;
use spr_client::{SolanaRpc, ZolanaClient};
use spr_keypair::ShieldedKeypair;
// The stateful wallet, with its transaction history, lives in spr-test-utils.
use spr_test_utils::wallet::{
    sync_wallet_with_config, PrivateTransaction, SyncWalletConfig, Wallet,
};

fn main() -> Result<()> {
    let context = setup()?;
    let client = context.client()?;
    run(&client, &cli_keypair()?).map(|_| ())
}

pub fn run(client: &ZolanaClient<SolanaRpc>, payer: &Keypair) -> Result<Vec<PrivateTransaction>> {
    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(payer)?;
    let assets = asset_registry(client)?;

    // Sync all transaction pages and resolve SPL asset registrations.
    let mut wallet = Wallet::new(sender.shielded_address()?, assets)
        .map_err(|e| anyhow!("create wallet: {e:?}"))?;
    let report = sync_wallet_with_config(
        &mut wallet,
        &sender,
        client,
        SyncWalletConfig {
            page_limit: 50,
            ..SyncWalletConfig::default()
        },
    )?;
    anyhow::ensure!(
        report.unknown_asset_ids.is_empty() && report.unknown_mints.is_empty(),
        "could not resolve SPL asset registrations"
    );

    for tx in wallet.private_transactions() {
        println!(
            "ok kind={:?} direction={:?} mint={} amount={} tx={}",
            tx.kind, tx.direction, tx.asset, tx.amount, tx.id.signature,
        );
    }
    Ok(wallet.private_transactions().to_vec())
}

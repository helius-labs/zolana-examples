use anyhow::Result;
use rust_client_example::{client, env_config};
use solana_signer::Signer;
use zolana_client::{ensure_registered, is_wallet_registered_sync};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet};

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = client(&cfg);
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // A private wallet is an in-memory object; there is nothing on-chain to
    // create for it.
    let _wallet = Wallet::new(keypair.shielded_address()?, AssetRegistry::default())?;

    // Publish the shielded keys to the on-chain registry so senders can route a
    // private transfer by Solana pubkey. Idempotent: it does nothing if the
    // record is already current.
    let registered_before = is_wallet_registered_sync(&client, cfg.payer.pubkey())?;
    ensure_registered(&client, &cfg.payer, &keypair)?;

    println!(
        "ok private wallet solana_address={} registered_before={registered_before}",
        cfg.payer.pubkey()
    );
    Ok(())
}

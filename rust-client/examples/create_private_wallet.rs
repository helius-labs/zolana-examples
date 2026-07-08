use anyhow::Result;
use rust_client_example::env_config;
use zolana_client::{create_private_wallet, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let seed = *payer.secret_bytes();
    let client = ZolanaClient::devnet(payer, &api_key);
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;

    // Create the wallet and register its address so others can send to it privately.
    let _wallet = create_private_wallet(
        client.rpc(),
        client.payer(),
        keypair,
        AssetRegistry::default(),
    )?;

    println!(
        "ok private wallet solana_address={}",
        client.payer().pubkey()
    );
    Ok(())
}

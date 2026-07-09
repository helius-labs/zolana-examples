use anyhow::Result;
use rust_client_example::env_config;
use solana_signer::Signer;
use zolana_client::{create_private_wallet, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Create a private wallet. This adds the wallet address to a lookup table
    // for private transfers.
    let _wallet = create_private_wallet(&rpc, &payer, keypair, AssetRegistry::default())?;

    println!("ok private wallet solana_address={}", payer.pubkey());
    Ok(())
}

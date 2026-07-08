use anyhow::Result;
use rust_client_example::{deposit_sol, env_config};
use zolana_client::{create_private_wallet, Rpc, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    // One ed25519 key signs both the Solana account and the private balance.
    let seed = *payer.secret_bytes();
    let client = ZolanaClient::devnet(payer, &api_key);
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let mut wallet = create_private_wallet(
        client.rpc(),
        client.payer(),
        keypair.clone(),
        AssetRegistry::default(),
    )?;

    // Setup: deposit SOL to the private balance.
    deposit_sol(&client, &keypair, &mut wallet, 5_000_000)?;

    // Query indexer for private balances of a wallet and decrypts the results
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = client
        .indexer()
        .get_encrypted_utxos_by_tags(tags, None, None)?;

    println!("ok query encrypted_matches={}", response.matches.len());
    Ok(())
}

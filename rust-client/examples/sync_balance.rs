use anyhow::Result;
use rust_client_example::{deposit_sol, env_config};
use zolana_client::{
    create_private_wallet, get_private_token_balances, sync_wallet, Rpc, ZolanaClient,
};
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

    // Sync the wallet, then read the private balance per asset.
    sync_wallet(&mut wallet, client.indexer())?;
    let balance = get_private_token_balances(&wallet)?;

    // The raw layer beneath sync: query the indexer for the wallet's
    // encrypted outputs by view tag. Sync runs this query over the wallet's
    // full tag set and decrypts the matches.
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = client
        .indexer()
        .get_encrypted_utxos_by_tags(tags, None, None)?;

    println!(
        "ok private_balance={balance:?} encrypted_matches={}",
        response.matches.len()
    );
    Ok(())
}

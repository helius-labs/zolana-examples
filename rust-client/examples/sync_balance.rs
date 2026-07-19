use anyhow::Result;
use rust_client_example::{env_config, setup_funded_sol_wallet};
use zolana_client::{get_private_token_balances, sync_wallet, Rpc, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Setup: Create a private wallet and deposit SOL into it.
    let mut wallet = setup_funded_sol_wallet(&rpc, &payer, rpc.tree(), &keypair, 5_000_000)?;

    // Sync the wallet, then read the private balance per asset.
    sync_wallet(&mut wallet, &rpc)?;
    let balance = get_private_token_balances(&wallet)?;

    // The raw layer beneath sync: query the indexer for encrypted entries
    // matching one of the wallet's tags. Sync runs this query over the
    // wallet's full tag set and decrypts the matches.
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = rpc.get_encrypted_utxos_by_tags(tags, None, None)?;

    println!(
        "ok private_balance={balance:?} encrypted_matches={}",
        response.matches.len()
    );
    Ok(())
}

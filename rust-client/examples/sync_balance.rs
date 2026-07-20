use anyhow::Result;
use rust_client_example::{authority, client, env_config, setup_funded_sol_wallet};
use zolana_client::{get_private_token_balances, sync_wallet, Rpc};
use zolana_keypair::ShieldedKeypair;

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = client(&cfg);
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register the wallet and deposit SOL into it.
    let mut wallet = setup_funded_sol_wallet(&client, &cfg.payer, &keypair, 5_000_000)?;

    // Sync the wallet, then read the private balance per asset.
    let authority = authority(&cfg.payer, &keypair);
    sync_wallet(&mut wallet, &authority, &client)?;
    let balance = get_private_token_balances(&wallet)?;

    // The raw layer beneath sync: query the indexer for encrypted entries
    // matching one of the wallet's tags. Sync runs this query over the wallet's
    // full tag set and decrypts the matches.
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = client.get_encrypted_utxos_by_tags(tags, None, None, None)?;

    println!(
        "ok private_balance={balance:?} encrypted_matches={}",
        response.matches.len()
    );
    Ok(())
}

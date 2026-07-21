use anyhow::Result;
use rust_client_example::{env_config, setup_funded_sol_wallet};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{get_private_token_balances, sync_wallet, Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::LocalWalletAuthority;

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register the wallet and deposit SOL into it.
    let mut wallet = setup_funded_sol_wallet(&client, &cfg.payer, &keypair, 5_000_000)?;

    // Sync the wallet, then read the private balance per asset.
    let authority = LocalWalletAuthority::new(
        Address::new_from_array(cfg.payer.pubkey().to_bytes()),
        &keypair,
    );
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

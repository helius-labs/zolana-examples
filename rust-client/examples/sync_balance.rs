use anyhow::{anyhow, Result};
use rust_client_example::{create_private_wallet, deposit_sol};
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use zolana_client::{Rpc, ZolanaClient};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Load .env if present.
    dotenvy::dotenv().ok();

    // Load the fee payer, then connect to devnet with one client.
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR").unwrap_or_else(|_| {
        format!(
            "{}/.config/solana/id.json",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let api_key = std::env::var("API_KEY").expect("set API_KEY");
    let mut client = ZolanaClient::devnet(payer, &api_key);
    let (rpc, indexer, _prover, payer) = client.parts();
    let tree: Pubkey = std::env::var("ZOLANA_TREE")
        .expect("set ZOLANA_TREE")
        .parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    let (keypair, mut wallet) = create_private_wallet(rpc, payer, AssetRegistry::default())?;

    // Setup: Deposit SOL to private balance
    deposit_sol(rpc, payer, tree, indexer, &keypair, &mut wallet, 5_000_000)?;

    // Query indexer for private balances of a wallet and decrypts the results
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = indexer.get_encrypted_utxos_by_tags(tags, None, None)?;

    println!("ok query encrypted_matches={}", response.matches.len());
    Ok(())
}

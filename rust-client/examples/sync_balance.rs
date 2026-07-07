use anyhow::{anyhow, Result};
use rust_client_example::deposit_sol;
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use zolana_client::{create_private_wallet, Rpc, ZolanaClient};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Load .env if present.
    dotenvy::dotenv().ok();

    // Load the fee payer, then connect to devnet with one client.
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let payer_path = shellexpand::tilde(&payer_path).into_owned();
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let api_key = std::env::var("API_KEY").expect("set API_KEY");
    let mut client = ZolanaClient::devnet(payer, &api_key);
    let (rpc, indexer, _prover, payer) = client.parts();
    let tree: Pubkey = std::env::var("ZOLANA_TREE")
        .expect("set ZOLANA_TREE")
        .parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // One ed25519 key signs both the Solana account and the private balance.
    let seed = *payer.secret_bytes();
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let mut wallet = create_private_wallet(rpc, payer, keypair.clone(), AssetRegistry::default())?;

    // Setup: deposit SOL to the private balance.
    deposit_sol(rpc, payer, tree, indexer, &keypair, &mut wallet, 5_000_000)?;

    // Query indexer for private balances of a wallet and decrypts the results
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = indexer.get_encrypted_utxos_by_tags(tags, None, None)?;

    println!("ok query encrypted_matches={}", response.matches.len());
    Ok(())
}

use anyhow::{anyhow, Result};
use rust_client_example::{create_private_wallet, deposit_sol};
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use zolana_client::{ProverClient, Rpc, SolanaRpc, ZolanaIndexer};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Connect to the devnet deployment.
    let indexer = ZolanaIndexer::new("http://202.8.10.77:8784/");
    let rpc_url = format!(
        "https://devnet.helius-rpc.com/?api-key={}",
        std::env::var("API_KEY").expect("set API_KEY"),
    );
    let mut rpc = SolanaRpc::new(rpc_url).with_indexer(indexer.clone());
    let _prover = ProverClient::new("http://202.8.10.77:3011".to_string());
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| format!("{}/.config/solana/id.json", std::env::var("HOME").unwrap_or_default()));
    let payer = read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree: Pubkey = std::env::var("ZOLANA_TREE").expect("set ZOLANA_TREE").parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    let (keypair, _funding, mut wallet) =
        create_private_wallet(&mut rpc, &payer, AssetRegistry::default())?;

    // Setup: Deposit SOL to private balance
    deposit_sol(&rpc, &payer, tree, &indexer, &keypair, &mut wallet, 5_000_000)?;
    deposit_sol(&rpc, &payer, tree, &indexer, &keypair, &mut wallet, 2_000_000)?;

    // Query indexer for private balances of a wallet and decrypts the results
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = indexer.get_encrypted_utxos_by_tags(tags, None, None)?;

    println!("ok query encrypted_matches={}", response.matches.len());
    Ok(())
}

use anyhow::{anyhow, Result};
use rust_client_example::create_private_wallet;
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{ProverClient, SolanaRpc, ZolanaIndexer};
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
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Create the keypair that owns the private balance, fund a Solana fee key, and
    // register the wallet address so senders can transfer to it privately. Senders
    // transfer privately to the regular Solana public key that serves as inbox for
    // the private wallet; if a public key is not registered, transfers resolve to a
    // private-to-public withdrawal.
    let (_keypair, funding, _wallet) =
        create_private_wallet(&mut rpc, &payer, AssetRegistry::default())?;

    println!("ok private wallet solana_address={}", funding.pubkey());
    Ok(())
}

use anyhow::{anyhow, Result};
use rust_client_example::create_private_wallet;
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::SolanaRpc;
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    // Load .env if present.
    dotenvy::dotenv().ok();

    // Connect to devnet.
    let rpc_url = format!(
        "https://devnet.helius-rpc.com/?api-key={}",
        std::env::var("API_KEY").expect("set API_KEY"),
    );
    let mut rpc = SolanaRpc::new(rpc_url);
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR").unwrap_or_else(|_| {
        format!(
            "{}/.config/solana/id.json",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Create a private wallet. This adds the wallet address to a lookup table for
    // private transfers.
    let (_keypair, _wallet) = create_private_wallet(&mut rpc, &payer, AssetRegistry::default())?;

    println!("ok private wallet solana_address={}", payer.pubkey());
    Ok(())
}

use anyhow::{anyhow, Result};
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{ensure_registered, ZolanaClient};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_transaction::{AssetRegistry, Wallet};

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
    let (rpc, _indexer, _prover, payer) = client.parts();
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;
    let seed = *payer.secret_bytes();
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let _wallet = Wallet::new(keypair.clone(), AssetRegistry::default())?;

    // Register the wallet address in a lookup table so others can send to it privately.
    ensure_registered(rpc, payer, &keypair)?;

    println!("ok private wallet solana_address={}", payer.pubkey());
    Ok(())
}

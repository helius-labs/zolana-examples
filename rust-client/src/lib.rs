//! Shared environment settings for the instruction example: the fee payer,
//! Helius RPC, Photon indexer, and prover.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use zolana_interface::DEFAULT_TREE_ADDRESS;
use zolana_keypair::{ShieldedAddress, ShieldedKeypair};

/// The RPC, Photon indexer, and prover the examples talk to.
pub const RPC_URL: &str = "https://devnet.helius-rpc.com";
pub const INDEXER_URL: &str = "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com";
pub const PROVER_URL: &str = "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com:3001";
// localnet: pub const RPC_URL: &str = "http://127.0.0.1:8899";
// localnet: pub const INDEXER_URL: &str = "http://127.0.0.1:8784";
// localnet: pub const PROVER_URL: &str = "http://127.0.0.1:3001";

/// Service URLs, the funded sender, and a fresh recipient address.
pub struct SetupContext {
    pub rpc_url: String,
    pub indexer_url: String,
    pub prover_url: String,
    pub tree: Address,
    pub sender: ShieldedKeypair,
    pub recipient_address: ShieldedAddress,
}

/// Read the environment settings: the fee payer (`ZOLANA_PAYER_KEYPAIR`,
/// defaults to the Solana CLI wallet) and the `API_KEY` for the Helius devnet
/// RPC. Toggle the `localnet:` lines to run against a local stack instead.
pub fn setup() -> Result<SetupContext> {
    dotenvy::dotenv().ok();
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let payer_path = shellexpand::tilde(&payer_path).into_owned();
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree = DEFAULT_TREE_ADDRESS
        .parse()
        .map_err(|e| anyhow!("parse tree address: {e}"))?;

    let api_key = std::env::var("API_KEY").map_err(|_| anyhow!("set API_KEY"))?;
    let rpc_url = format!("{RPC_URL}/?api-key={api_key}");
    // localnet: let rpc_url = RPC_URL.to_string();

    let sender = ShieldedKeypair::from_solana_keypair(&payer)?;
    let recipient_address =
        ShieldedKeypair::from_solana_keypair(&Keypair::new())?.shielded_address()?;

    Ok(SetupContext {
        rpc_url,
        indexer_url: INDEXER_URL.to_string(),
        prover_url: PROVER_URL.to_string(),
        tree,
        sender,
        recipient_address,
    })
}

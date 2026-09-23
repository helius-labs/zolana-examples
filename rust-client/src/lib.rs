//! Shared environment settings for the instruction example: the fee payer,
//! Helius RPC, Photon indexer, and prover.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_signer::Signer;
use solana_system_interface::instruction::create_account;
use spl_token_interface::instruction::{initialize_account3, initialize_mint2, mint_to};
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::{
    pda, SPL_TOKEN_ACCOUNT_LEN, SPL_TOKEN_MINT_ACCOUNT_LEN, SPL_TOKEN_PROGRAM_ID,
};

/// The RPC, Photon indexer, and prover the examples talk to.
pub const RPC_URL: &str = "https://devnet.helius-rpc.com";
pub const INDEXER_URL: &str = "https://d2xah7tnhdhcom.cloudfront.net";
pub const PROVER_URL: &str = "https://d21ni15goiip6l.cloudfront.net";
// localnet: pub const RPC_URL: &str = "http://127.0.0.1:8899";
// localnet: pub const INDEXER_URL: &str = "http://127.0.0.1:8784";
// localnet: pub const PROVER_URL: &str = "http://127.0.0.1:3001";

/// Service URLs and the default tree.
pub struct SetupContext {
    pub rpc_url: String,
    pub indexer_url: String,
    pub prover_url: String,
    pub tree: Address,
}

/// Read the environment settings and the `API_KEY` for the Helius devnet RPC.
/// Defaults are Helius plus the Photon/prover HTTPS endpoints. Toggle the `localnet:`
/// lines to run against a local stack instead.
pub fn setup() -> Result<SetupContext> {
    dotenvy::dotenv().ok();
    let tree = pda::tree(0);
    let api_key = std::env::var("API_KEY").map_err(|_| anyhow!("set API_KEY"))?;
    let rpc_url = format!("{RPC_URL}/?api-key={api_key}");
    // localnet: let rpc_url = RPC_URL.to_string();

    Ok(SetupContext {
        rpc_url,
        indexer_url: INDEXER_URL.to_string(),
        prover_url: PROVER_URL.to_string(),
        tree,
    })
}

/// The Solana CLI wallet (`ZOLANA_PAYER_KEYPAIR`, defaults to
/// `~/.config/solana/id.json`).
pub fn cli_keypair() -> Result<Keypair> {
    let path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let path = shellexpand::tilde(&path).into_owned();
    read_keypair_file(&path).map_err(|e| anyhow!("load keypair {path}: {e}"))
}

/// Create a test mint and fund the payer's token account. Returns their addresses.
pub fn setup_test_token(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    amount: u64,
) -> Result<(Address, Address)> {
    let payer_pubkey = payer.pubkey();
    let mint = Keypair::new();
    let source_token = Keypair::new();
    let token_program = Address::new_from_array(SPL_TOKEN_PROGRAM_ID);
    let mint_rent = client.get_minimum_balance_for_rent_exemption(SPL_TOKEN_MINT_ACCOUNT_LEN)?;
    let token_rent = client.get_minimum_balance_for_rent_exemption(SPL_TOKEN_ACCOUNT_LEN)?;
    client.create_and_send_transaction(
        &[
            create_account(
                &payer_pubkey,
                &mint.pubkey(),
                mint_rent,
                SPL_TOKEN_MINT_ACCOUNT_LEN as u64,
                &token_program,
            ),
            initialize_mint2(&token_program, &mint.pubkey(), &payer_pubkey, None, 9)?,
            create_account(
                &payer_pubkey,
                &source_token.pubkey(),
                token_rent,
                SPL_TOKEN_ACCOUNT_LEN as u64,
                &token_program,
            ),
            initialize_account3(
                &token_program,
                &source_token.pubkey(),
                &mint.pubkey(),
                &payer_pubkey,
            )?,
            mint_to(
                &token_program,
                &mint.pubkey(),
                &source_token.pubkey(),
                &payer_pubkey,
                &[],
                amount,
            )?,
        ],
        payer_pubkey,
        &[payer, &mint, &source_token],
        client.compute_budget(),
    )?;

    Ok((mint.pubkey(), source_token.pubkey()))
}

//! Shared setup for the examples: the fee payer, Helius RPC, Photon indexer,
//! prover, and funded test tokens.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction::create_account;
use spl_token_interface::instruction::{initialize_account3, initialize_mint2, mint_to};
use spr_client::{ProgramAccountsFilter, ProofDataSource, Rpc, SolanaRpc, ZolanaClient};
use spr_interface::{
    pda,
    state::{discriminator::SPL_ASSET_REGISTRY, SplAssetRegistry},
    PROGRAM_ID_PUBKEY, SPL_TOKEN_ACCOUNT_LEN, SPL_TOKEN_MINT_ACCOUNT_LEN,
};
use spr_transaction::AssetRegistry;

/// The RPC, Photon indexer, and prover the examples talk to.
pub const RPC_URL: &str = "https://devnet.helius-rpc.com";
pub const INDEXER_URL: &str = "https://d21d98xh5lgv3b.cloudfront.net";
pub const PROVER_URL: &str = "https://d1unet9hn0mci0.cloudfront.net";
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

impl SetupContext {
    pub fn client(&self) -> Result<ZolanaClient<SolanaRpc>> {
        Ok(ZolanaClient::from_urls(
            SolanaRpc::new(self.rpc_url.clone()),
            &self.indexer_url,
            self.prover_url.clone(),
        )?
        .with_proof_data_source(ProofDataSource::Prover))
    }
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

/// Load the registered SPL mints before scanning notes that carry a mint address.
pub fn asset_registry(client: &ZolanaClient<SolanaRpc>) -> Result<AssetRegistry> {
    let mut assets = AssetRegistry::default();
    let filter =
        ProgramAccountsFilter::new(SplAssetRegistry::SIZE).with_memcmp(0, vec![SPL_ASSET_REGISTRY]);
    for (address, account) in client
        .rpc()
        .get_program_accounts_filtered(PROGRAM_ID_PUBKEY, &filter)?
    {
        let registry = SplAssetRegistry::from_account_bytes(&account.data)
            .map_err(|error| anyhow!("decode SPL asset registry: {error:?}"))?;
        anyhow::ensure!(
            account.owner == PROGRAM_ID_PUBKEY
                && address == pda::spl_asset_registry(&registry.mint),
            "invalid SPL asset registry account"
        );
        assets.insert(registry.asset_id, registry.mint)?;
    }
    Ok(assets)
}

/// Test mint and the payer's funded token account.
pub struct TestToken {
    pub mint: Address,
    pub source_token: Address,
}

/// Create a test mint and fund the payer's token account.
pub fn setup_test_token(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    amount: u64,
) -> Result<TestToken> {
    let payer_pubkey = payer.pubkey();
    let mint = Keypair::new();
    let source_token = Keypair::new();
    let token_program = pda::spl_token_program_id();
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

    Ok(TestToken {
        mint: mint.pubkey(),
        source_token: source_token.pubkey(),
    })
}

/// Slot the confirmed transaction landed in, which drives the indexer
/// freshness gate on the fetches that read the transaction back.
pub fn landed_slot(client: &ZolanaClient<SolanaRpc>, signature: Signature) -> Result<u64> {
    client
        .get_signature_statuses(vec![signature])?
        .first()
        .and_then(|status| status.as_ref())
        .map(|status| status.slot)
        .ok_or_else(|| anyhow!("transaction status missing after confirmation"))
}

//! Test scaffolding for the client examples: read the environment settings,
//! build the client, fund fresh keys, and deposit-as-setup shorthands.
//! Production integrators bring funded keys; nothing here is needed in
//! production.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::DEFAULT_TREE_ADDRESS;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};
use zolana_wallet::{
    create_deposit, ensure_registered, sync_wallet, DepositParams, LocalWalletAuthority,
};

/// The RPC, Photon indexer, and prover the examples talk to.
pub const RPC_URL: &str = "https://devnet.helius-rpc.com";
pub const INDEXER_URL: &str = "http://202.8.10.77:8784";
pub const PROVER_URL: &str = "http://202.8.10.77:3011";
// localnet: pub const RPC_URL: &str = "http://127.0.0.1:8899";
// localnet: pub const INDEXER_URL: &str = "http://127.0.0.1:8784";
// localnet: pub const PROVER_URL: &str = "http://127.0.0.1:3001";

/// A private wallet registered on devnet; the examples send to it.
pub const DEFAULT_RECIPIENT: &str = "DNRJcGsGR6SGEYuNAaRtbZ8a86snwVcH5CJh1VcSLxx";

/// Service URLs and the fee payer.
pub struct Config {
    pub payer: Keypair,
    pub rpc_url: String,
    pub indexer_url: String,
    pub prover_url: String,
    pub tree: Address,
}

impl Config {
    /// The interface instruction builders use Solana's `Pubkey` type, while
    /// the client stores the same tree address as `Address`.
    pub fn tree_pubkey(&self) -> Pubkey {
        Pubkey::new_from_array(self.tree.to_bytes())
    }
}

/// Read the environment settings: the fee payer (`ZOLANA_PAYER_KEYPAIR`,
/// defaults to the Solana CLI wallet) and the `API_KEY` for the Helius devnet
/// RPC. Toggle the `localnet:` lines to run against a local stack instead.
pub fn env_config() -> Result<Config> {
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

    Ok(Config {
        payer,
        rpc_url,
        indexer_url: INDEXER_URL.to_string(),
        prover_url: PROVER_URL.to_string(),
        tree,
    })
}

/// Move `lamports` from the payer to `to`. Localnet keys start empty, so the
/// payer funds the keys the examples need.
fn fund_key(rpc: &impl Rpc, payer: &Keypair, to: &Pubkey, lamports: u64) -> Result<Signature> {
    let ix = solana_system_interface::instruction::transfer(&payer.pubkey(), to, lamports);
    Ok(rpc.create_and_send_transaction(&[ix], payer.pubkey(), &[payer])?)
}

/// Fund a fresh test recipient and register its private wallet on-chain. The
/// recipient owns and pays for its own registration. Use this on localnet,
/// where `DEFAULT_RECIPIENT` is not registered.
pub fn create_test_recipient(rpc: &ZolanaClient<SolanaRpc>, payer: &Keypair) -> Result<Pubkey> {
    let recipient = Keypair::new();
    fund_key(rpc, payer, &recipient.pubkey(), 20_000_000)?;
    let shielded_keypair = ShieldedKeypair::from_solana_keypair(&recipient)?;
    ensure_registered(rpc, &recipient, &shielded_keypair)?;
    Ok(recipient.pubkey())
}

/// Register a private wallet for `keypair` and deposit `amount` of SOL into it.
pub fn setup_funded_sol_wallet(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    amount: u64,
) -> Result<Wallet> {
    ensure_registered(client, payer, keypair)?;
    let mut wallet = Wallet::new(keypair.shielded_address()?, AssetRegistry::default())?;
    deposit_sol(client, payer, keypair, &mut wallet, amount)?;
    Ok(wallet)
}

/// Wait until the indexer has picked up the deposit's output for `tag`, then
/// sync the wallet so the deposited balance appears.
pub fn sync_after_deposit(
    client: &ZolanaClient<SolanaRpc>,
    wallet: &mut Wallet,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    tag: [u8; 32],
    signature: Signature,
) -> Result<()> {
    for _ in 0..30 {
        let indexed = client
            .get_encrypted_utxos_by_tags(vec![tag], None, Some(50), None)?
            .matches
            .iter()
            .any(|m| m.tx_signature == signature);
        if indexed {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let authority = LocalWalletAuthority::new(payer.pubkey(), keypair);
    sync_wallet(wallet, &authority, client)?;
    Ok(())
}

/// Setup shorthand for depositing into a wallet: prepare the deposit, send it,
/// then wait for the indexer and sync (a self-deposit, so the wallet is the
/// recipient's).
#[allow(clippy::too_many_arguments)]
fn deposit(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    asset: Address,
    amount: u64,
    spl_token_account: Option<Pubkey>,
) -> Result<()> {
    let prepared = create_deposit(DepositParams {
        recipient: &keypair.shielded_address()?,
        asset,
        amount,
        spl_token_account,
        spl_token_program: None,
        memo: None,
    })?;
    let tree = DEFAULT_TREE_ADDRESS
        .parse()
        .map_err(|e| anyhow!("parse tree address: {e}"))?;
    let signature = prepared.send(client, payer, tree, payer)?;
    sync_after_deposit(
        client,
        wallet,
        payer,
        keypair,
        prepared.view_tag(),
        signature,
    )?;
    Ok(())
}

/// Move `amount` of SOL into the private balance of `keypair`.
fn deposit_sol(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    amount: u64,
) -> Result<()> {
    deposit(client, payer, keypair, wallet, SOL_MINT, amount, None)
}

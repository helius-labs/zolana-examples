//! Test scaffolding for the localnet examples: read the environment settings,
//! build the client, create a test SPL mint with its interface PDA, fund fresh
//! keys, and deposit-as-setup shorthands. Production integrators bring an
//! existing mint and funded keys; nothing here is needed in production.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_deposit, ensure_registered, DepositParams, Rpc,
    SolanaRpc, ZolanaClient,
};
use zolana_interface::DEFAULT_TREE_ADDRESS;
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::{
    create_mint, create_spl_interface, create_token_account, ensure_asset_counter, mint_to,
};
use zolana_transaction::{AssetRegistry, LocalWalletAuthority, Wallet, SOL_MINT};

/// The RPC, Photon indexer, and prover the examples talk to.
pub const RPC_URL: &str = "https://devnet.helius-rpc.com";
pub const INDEXER_URL: &str = "http://202.8.10.77:8784";
pub const PROVER_URL: &str = "http://202.8.10.77:3011";
// localnet: pub const RPC_URL: &str = "http://127.0.0.1:8899";
// localnet: pub const INDEXER_URL: &str = "http://127.0.0.1:8784";
// localnet: pub const PROVER_URL: &str = "http://127.0.0.1:3001";

/// An SPL asset registered for private balances and transactions.
#[derive(Clone, Copy)]
pub struct SplAsset {
    pub mint: Pubkey,
    pub user_token: Pubkey,
}

/// Service URLs and the fee payer.
pub struct Config {
    pub payer: Keypair,
    pub rpc_url: String,
    pub indexer_url: String,
    pub prover_url: String,
    pub tree: Address,
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

/// Build the client from a config.
pub fn client(cfg: &Config) -> ZolanaClient<SolanaRpc> {
    ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    )
}

/// Derive the shielded keypair from a Solana keypair: one ed25519 key signs
/// both the Solana account and the private balance.
pub fn shielded_keypair(payer: &Keypair) -> Result<ShieldedKeypair> {
    let seed: [u8; 32] = payer.to_bytes()[..32]
        .try_into()
        .map_err(|_| anyhow!("keypair seed"))?;
    Ok(ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?)
}

/// The authority that signs a wallet's balance reads and private transactions.
pub fn authority<'a>(payer: &Keypair, keypair: &'a ShieldedKeypair) -> LocalWalletAuthority<'a> {
    LocalWalletAuthority::new(Address::new_from_array(payer.pubkey().to_bytes()), keypair)
}

/// The tree as a `Pubkey`, for the instruction builders that want one.
pub fn tree_pubkey(client: &ZolanaClient<SolanaRpc>) -> Pubkey {
    Pubkey::new_from_array(client.tree().to_bytes())
}

/// Move `lamports` from the payer to `to`. Localnet keys start empty, so the
/// payer funds the keys the examples need.
pub fn fund_key(
    rpc: &impl Rpc,
    payer: &Keypair,
    to: &Pubkey,
    lamports: u64,
) -> Result<Signature> {
    let ix = solana_system_interface::instruction::transfer(&payer.pubkey(), to, lamports);
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    Ok(rpc.create_and_send_transaction(&[ix], payer_address, &[payer])?)
}

/// Create a test mint, register its interface PDA, open a funded token account
/// for the payer, and map the mint to its assigned asset id.
pub fn register_asset(rpc: &impl Rpc, payer: &Keypair) -> Result<(SplAsset, AssetRegistry)> {
    let mint = create_mint(rpc, payer)?;
    ensure_asset_counter(rpc, payer)?;
    create_spl_interface(rpc, payer, &mint)?;
    let user_token = create_token_account(rpc, payer, &mint, &payer.pubkey())?;

    let asset_id = fetch_asset_id(rpc, &mint)?;
    let mut registry = AssetRegistry::default();
    registry
        .insert(asset_id, Address::new_from_array(mint.to_bytes()))
        .map_err(|e| anyhow!("register SPL asset: {e}"))?;

    Ok((SplAsset { mint, user_token }, registry))
}

/// Read the asset id the program assigned to `mint` from its registry PDA.
fn fetch_asset_id(rpc: &impl Rpc, mint: &Pubkey) -> Result<u64> {
    let registry = zolana_interface::pda::spl_asset_registry(mint);
    let account = rpc
        .get_account(Address::new_from_array(registry.to_bytes()))?
        .ok_or_else(|| anyhow!("SPL asset registry not found for mint {mint}"))?;
    let bytes: [u8; 8] = account.data[40..48]
        .try_into()
        .map_err(|_| anyhow!("read asset id"))?;
    Ok(u64::from_le_bytes(bytes))
}

/// A funded test recipient with its private wallet.
pub struct TestRecipient {
    pub keypair: Keypair,
    pub shielded_keypair: ShieldedKeypair,
    pub wallet: Wallet,
}

/// Fund a fresh test recipient and register its private wallet on-chain. The
/// recipient owns and pays for its own registration. One ed25519 key signs both
/// the Solana account and the private balance.
pub fn create_test_recipient(
    rpc: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    registry: AssetRegistry,
) -> Result<TestRecipient> {
    let recipient = Keypair::new();
    fund_key(rpc, payer, &recipient.pubkey(), 20_000_000)?;
    let shielded_keypair = shielded_keypair(&recipient)?;
    ensure_registered(rpc, &recipient, &shielded_keypair)?;
    let wallet = Wallet::new(shielded_keypair.shielded_address()?, registry)?;
    Ok(TestRecipient {
        keypair: recipient,
        shielded_keypair,
        wallet,
    })
}

/// A private wallet funded with a test asset.
pub struct FundedWallet {
    pub asset: SplAsset,
    pub registry: AssetRegistry,
    pub wallet: Wallet,
}

/// Register a test asset and a private wallet for `keypair`, then deposit
/// `amount` of the asset into the wallet.
pub fn setup_funded_wallet(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    amount: u64,
) -> Result<FundedWallet> {
    let (asset, registry) = register_asset(client, payer)?;
    ensure_registered(client, payer, keypair)?;
    let mut wallet = Wallet::new(keypair.shielded_address()?, registry.clone())?;
    deposit_spl(client, payer, keypair, &mut wallet, &asset, amount)?;
    Ok(FundedWallet {
        asset,
        registry,
        wallet,
    })
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

/// Create a fresh test recipient and its token account for `mint`.
pub fn create_test_recipient_token_account(
    rpc: &impl Rpc,
    payer: &Keypair,
    mint: &Pubkey,
) -> Result<(Keypair, Pubkey)> {
    let recipient = Keypair::new();
    let (_signature, token_account) =
        create_associated_token_account(rpc, payer, &recipient.pubkey(), mint)?;
    Ok((recipient, token_account))
}

/// Wait until the indexer has picked up the deposit's output for `tag`, then
/// sync the wallet so the deposited balance appears.
fn sync_after_deposit(
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
    let authority = authority(payer, keypair);
    zolana_client::sync_wallet(wallet, &authority, client)?;
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
        memo: None,
    })?;
    let signature = prepared.send(client, payer, tree_pubkey(client), payer)?;
    sync_after_deposit(client, wallet, payer, keypair, prepared.view_tag(), signature)?;
    Ok(())
}

/// Move `amount` of SOL into the private balance of `keypair`.
pub fn deposit_sol(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    amount: u64,
) -> Result<()> {
    deposit(client, payer, keypair, wallet, SOL_MINT, amount, None)
}

/// Fund the token account, then move `amount` of an SPL asset into the private
/// balance of `keypair`.
pub fn deposit_spl(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    asset: &SplAsset,
    amount: u64,
) -> Result<()> {
    mint_to(client, payer, &asset.mint, &asset.user_token, amount)?;
    let mint = Address::new_from_array(asset.mint.to_bytes());
    deposit(
        client,
        payer,
        keypair,
        wallet,
        mint,
        amount,
        Some(asset.user_token),
    )
}

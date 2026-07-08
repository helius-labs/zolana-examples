//! Test scaffolding for the devnet examples: read the `.env` settings, create
//! a test SPL mint with its interface PDA, fund fresh keys, and
//! deposit-as-setup shorthands. Production integrators bring an existing mint
//! and funded keys; nothing here is needed in production.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_deposit, create_private_wallet, register_spl_interface,
    CreateDeposit, Rpc, ZolanaClient,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::{create_mint, create_token_account, mint_to};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};

/// An SPL asset registered for private balances and transactions.
#[derive(Clone, Copy)]
pub struct SplAsset {
    pub mint: Pubkey,
    pub user_token: Pubkey,
}

/// Read the `.env` settings: the fee payer (`ZOLANA_PAYER_KEYPAIR`, defaults
/// to the Solana CLI wallet) and the Helius API key (`API_KEY`).
pub fn env_config() -> Result<(Keypair, String)> {
    dotenvy::dotenv().ok();
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let payer_path = shellexpand::tilde(&payer_path).into_owned();
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let api_key = std::env::var("API_KEY").map_err(|_| anyhow!("set API_KEY"))?;
    Ok((payer, api_key))
}

/// Move `lamports` from the payer to `to`. Devnet has no faucet, so the payer
/// funds the keys the examples need.
pub fn fund_key(client: &ZolanaClient, to: &Pubkey, lamports: u64) -> Result<Signature> {
    let payer = client.payer();
    let ix = solana_system_interface::instruction::transfer(&payer.pubkey(), to, lamports);
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    Ok(client
        .rpc()
        .create_and_send_transaction(&[ix], payer_address, &[payer])?)
}

/// Create test mint with interface PDA for private balances and transactions.
pub fn register_asset(client: &ZolanaClient) -> Result<(SplAsset, AssetRegistry)> {
    let rpc = client.rpc();
    let payer = client.payer();
    let mint = create_mint(rpc, payer)?;
    let asset_id = register_spl_interface(rpc, payer, mint)?;
    let user_token = create_token_account(rpc, payer, &mint, &payer.pubkey())?;

    let mut registry = AssetRegistry::default();
    registry
        .insert(asset_id, Address::new_from_array(mint.to_bytes()))
        .map_err(|e| anyhow!("register SPL asset: {e}"))?;

    Ok((SplAsset { mint, user_token }, registry))
}

/// Fund a fresh test recipient and create its private wallet. The recipient
/// owns and pays for its own registration. One ed25519 key signs both the
/// Solana account and the private balance.
pub fn create_test_recipient(
    client: &ZolanaClient,
    registry: AssetRegistry,
) -> Result<(Keypair, ShieldedKeypair, Wallet)> {
    let recipient = Keypair::new();
    fund_key(client, &recipient.pubkey(), 20_000_000)?;
    let keypair = ShieldedKeypair::from_ed25519(recipient.secret_bytes(), ViewingKey::new())?;
    let wallet = create_private_wallet(client.rpc(), &recipient, keypair.clone(), registry)?;
    Ok((recipient, keypair, wallet))
}

/// Create a test asset and a private wallet from `seed`, then deposit
/// `amount` of the asset into the wallet.
pub fn setup_funded_wallet(
    client: &ZolanaClient,
    seed: &[u8; 32],
    amount: u64,
) -> Result<(SplAsset, AssetRegistry, ShieldedKeypair, Wallet)> {
    let (asset, registry) = register_asset(client)?;
    let keypair = ShieldedKeypair::from_ed25519(seed, ViewingKey::new())?;
    let mut wallet = create_private_wallet(
        client.rpc(),
        client.payer(),
        keypair.clone(),
        registry.clone(),
    )?;
    deposit_spl(client, &keypair, &mut wallet, &asset, amount)?;
    Ok((asset, registry, keypair, wallet))
}

/// Create a fresh test recipient and its token account for `mint`.
pub fn create_test_recipient_token_account(
    client: &ZolanaClient,
    mint: &Pubkey,
) -> Result<(Keypair, Pubkey)> {
    let recipient = Keypair::new();
    let (_signature, token_account) =
        create_associated_token_account(client.rpc(), client.payer(), &recipient.pubkey(), mint)?;
    Ok((recipient, token_account))
}

/// Setup shorthand for the SDK calls shown in the deposit example:
/// `create_deposit` + `send` + `wait_until_synced` (self-deposit, so the
/// wallet is the recipient's).
fn deposit(
    client: &ZolanaClient,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    asset: Address,
    amount: u64,
    spl_token_account: Option<Pubkey>,
) -> Result<()> {
    let prepared = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset,
        amount,
        spl_token_account,
        memo: None,
    })?;
    let signature = prepared.send(client.rpc(), client.payer(), client.tree(), client.payer())?;
    prepared.wait_until_synced(wallet, client.indexer(), signature)?;
    Ok(())
}

/// Move `amount` of SOL into the private balance of `keypair`.
pub fn deposit_sol(
    client: &ZolanaClient,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    amount: u64,
) -> Result<()> {
    deposit(client, keypair, wallet, SOL_MINT, amount, None)
}

/// Fund the token account, then move `amount` of an SPL asset into the private
/// balance of `keypair`.
pub fn deposit_spl(
    client: &ZolanaClient,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    asset: &SplAsset,
    amount: u64,
) -> Result<()> {
    mint_to(
        client.rpc(),
        client.payer(),
        &asset.mint,
        &asset.user_token,
        amount,
    )?;
    deposit(
        client,
        keypair,
        wallet,
        Address::new_from_array(asset.mint.to_bytes()),
        amount,
        Some(asset.user_token),
    )
}

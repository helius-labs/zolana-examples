//! Test scaffolding for the devnet examples: create a test SPL mint with its
//! interface PDA, fund fresh keys, and deposit-as-setup shorthands. Production
//! integrators bring an existing mint and funded keys; nothing here is needed
//! in production.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{
    create_deposit, register_spl_interface, CreateDeposit, Rpc, SolanaRpc, ZolanaIndexer,
};
use zolana_keypair::ShieldedKeypair;
use zolana_test_utils::spl::{create_mint, create_token_account, mint_to};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};

/// An SPL asset registered for private balances and transactions.
#[derive(Clone, Copy)]
pub struct SplAsset {
    pub mint: Pubkey,
    pub user_token: Pubkey,
}

/// Move `lamports` from the payer to `to`. Devnet has no faucet, so the payer
/// funds the keys the examples need.
pub fn fund_key(
    rpc: &SolanaRpc,
    payer: &Keypair,
    to: &Pubkey,
    lamports: u64,
) -> Result<Signature> {
    let ix = solana_system_interface::instruction::transfer(&payer.pubkey(), to, lamports);
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    Ok(rpc.create_and_send_transaction(&[ix], payer_address, &[payer])?)
}

/// Create test mint with interface PDA for private balances and transactions.
pub fn register_asset(rpc: &SolanaRpc, payer: &Keypair) -> Result<(SplAsset, AssetRegistry)> {
    let mint = create_mint(rpc, payer)?;
    let asset_id = register_spl_interface(rpc, payer, mint)?;
    let user_token = create_token_account(rpc, payer, &mint, &payer.pubkey())?;

    let mut registry = AssetRegistry::default();
    registry
        .insert(asset_id, Address::new_from_array(mint.to_bytes()))
        .map_err(|e| anyhow!("register SPL asset: {e}"))?;

    Ok((SplAsset { mint, user_token }, registry))
}

/// Setup shorthand for the two SDK calls shown in the deposit example:
/// `create_deposit` + `send_and_sync`.
#[allow(clippy::too_many_arguments)]
fn deposit(
    rpc: &SolanaRpc,
    payer: &Keypair,
    tree: Pubkey,
    indexer: &ZolanaIndexer,
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
    prepared.send_and_sync(rpc, payer, tree, payer, wallet, indexer)?;
    Ok(())
}

/// Move `amount` of SOL into the private balance of `keypair`.
pub fn deposit_sol(
    rpc: &SolanaRpc,
    payer: &Keypair,
    tree: Pubkey,
    indexer: &ZolanaIndexer,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    amount: u64,
) -> Result<()> {
    deposit(
        rpc, payer, tree, indexer, keypair, wallet, SOL_MINT, amount, None,
    )
}

/// Fund the token account, then move `amount` of an SPL asset into the private
/// balance of `keypair`.
#[allow(clippy::too_many_arguments)]
pub fn deposit_spl(
    rpc: &SolanaRpc,
    payer: &Keypair,
    tree: Pubkey,
    indexer: &ZolanaIndexer,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    asset: &SplAsset,
    amount: u64,
) -> Result<()> {
    mint_to(rpc, payer, &asset.mint, &asset.user_token, amount)?;
    deposit(
        rpc,
        payer,
        tree,
        indexer,
        keypair,
        wallet,
        Address::new_from_array(asset.mint.to_bytes()),
        amount,
        Some(asset.user_token),
    )
}

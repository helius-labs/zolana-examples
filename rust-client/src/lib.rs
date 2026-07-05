//! Setup helpers for examples: create a test SPL mint with interface PDA, fund a
//! fee key, create a private wallet, and deposit a balance.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;
use zolana_client::{create_deposit, sync_wallet, CreateDeposit, Rpc, SolanaRpc, ZolanaIndexer};
use zolana_interface::{
    instruction::{CreateAssetCounter, CreateSplInterface},
    pda,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::{create_mint, create_token_account, mint_to};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};

/// SOL occupies asset id 1; the first registered SPL mint gets id 2.
const FIRST_SPL_ASSET_ID: u64 = 2;

/// An SPL asset registered for private balances and transactions.
#[derive(Clone, Copy)]
pub struct SplAsset {
    pub mint: Pubkey,
    pub user_token: Pubkey,
}

fn send(
    rpc: &mut SolanaRpc,
    ixs: &[Instruction],
    payer: &Pubkey,
    signers: &[&Keypair],
) -> Result<Signature> {
    let (blockhash, _) = rpc.get_latest_blockhash()?;
    let message = Message::new(ixs, Some(payer));
    let transaction = Transaction::new(signers, message, blockhash);
    Ok(rpc.send_transaction(&transaction)?)
}

/// System-program transfer, hand-built so the crate needs no extra dependency.
fn system_transfer_ix(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = vec![2, 0, 0, 0];
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: Pubkey::default(),
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

/// Move `lamports` from the payer to `to`. Devnet has no faucet, so the payer
/// funds the keys the examples need.
pub fn fund_key(
    rpc: &mut SolanaRpc,
    payer: &Keypair,
    to: &Pubkey,
    lamports: u64,
) -> Result<Signature> {
    let ix = system_transfer_ix(&payer.pubkey(), to, lamports);
    send(rpc, &[ix], &payer.pubkey(), &[payer])
}

/// Create test mint with interface PDA for private balances and transactions.
pub fn register_asset(rpc: &mut SolanaRpc, payer: &Keypair) -> Result<(SplAsset, AssetRegistry)> {
    let mint = create_mint(rpc, payer)?;

    // The asset counter is a one-time singleton; create it only if missing.
    let counter_addr = Address::new_from_array(pda::spl_asset_counter().to_bytes());
    if rpc.get_account(counter_addr)?.is_none() {
        let ix = CreateAssetCounter {
            authority: payer.pubkey(),
        }
        .instruction();
        send(rpc, &[ix], &payer.pubkey(), &[payer])?;
    }

    let ix = CreateSplInterface {
        authority: payer.pubkey(),
        mint,
    }
    .instruction();
    send(rpc, &[ix], &payer.pubkey(), &[payer])?;

    let user_token = create_token_account(rpc, payer, &mint, &payer.pubkey())?;

    let mut registry = AssetRegistry::default();
    registry
        .insert(FIRST_SPL_ASSET_ID, Address::new_from_array(mint.to_bytes()))
        .map_err(|e| anyhow!("register SPL asset: {e}"))?;

    Ok((SplAsset { mint, user_token }, registry))
}

/// Create a private wallet owned by `owner` and register its address for private
/// transfers. `owner` pays for its own registration. Pass
/// `AssetRegistry::default()` for a SOL-only wallet.
pub fn create_private_wallet(
    rpc: &mut SolanaRpc,
    owner: &Keypair,
    registry: AssetRegistry,
) -> Result<(ShieldedKeypair, Wallet)> {
    // One ed25519 key signs both the Solana account and the private balance.
    let seed = *owner.secret_bytes();
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let wallet = Wallet::new(keypair.clone(), registry)?;
    zolana_client::ensure_registered(rpc, owner, &keypair)?;
    Ok((keypair, wallet))
}

/// Move `amount` of `asset` into the private balance of `keypair` and refresh
/// `wallet`. Pass `Some` token account for an SPL deposit, or `None` for SOL.
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
    let _signature = prepared.send(rpc, payer, tree, payer)?;
    // Sync the private balance.
    sync_wallet(wallet, indexer)?;
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

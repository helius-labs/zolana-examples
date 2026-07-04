//! Shared setup for the client examples. `setup` connects to an existing devnet
//! deployment and returns a [`Client`] and a [`Localnet`]. The harness holds only
//! what a real app never writes: the connection wiring, a throwaway SPL mint to
//! register, and the deposit seeding the transfer, withdraw, and sync examples
//! need. Each example holds the SDK call it demonstrates.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;
use zolana_client::{
    create_deposit, sync_wallet, CreateDeposit, ProverClient, Rpc, SolanaRpc, ZolanaIndexer,
};
use zolana_interface::{
    instruction::{CreateAssetCounter, CreateSplInterface},
    pda, SHIELDED_POOL_PROGRAM_ID,
};
use zolana_keypair::ShieldedKeypair;
use zolana_test_utils::spl::{create_mint, create_token_account, mint_to};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};
use zolana_user_registry_interface::user_registry_program_id;

const DEFAULT_INDEXER_URL: &str = "http://202.8.10.77:8784";

/// SOL occupies asset id 1; the first registered SPL mint gets id 2.
const FIRST_SPL_ASSET_ID: u64 = 2;

/// An SPL asset registered for private balances and transactions.
#[derive(Clone, Copy)]
pub struct SplAsset {
    pub mint: Pubkey,
    pub user_token: Pubkey,
}

/// The handles an app holds to run every operation.
pub struct Client {
    pub rpc: SolanaRpc,
    pub indexer: ZolanaIndexer,
    pub prover: ProverClient,
    pub tree: Pubkey,
    pub payer: Keypair,
}

/// Assets the wallet knows about, and the SPL mints registered this run.
pub struct Localnet {
    assets: AssetRegistry,
    spls: Vec<SplAsset>,
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

/// Load the fee payer from the Solana CLI wallet (override with
/// `ZOLANA_PAYER_KEYPAIR`). It must already hold SOL.
fn load_payer() -> Result<Keypair> {
    let path = std::env::var("ZOLANA_PAYER_KEYPAIR").unwrap_or_else(|_| {
        format!(
            "{}/.config/solana/id.json",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    solana_keypair::read_keypair_file(&path).map_err(|e| anyhow!("load payer keypair {path}: {e}"))
}

/// Devnet RPC: an explicit URL, else a Helius endpoint built from `API_KEY`.
fn rpc_url() -> String {
    std::env::var("ZOLANA_RPC_URL").unwrap_or_else(|_| {
        let key = std::env::var("API_KEY").unwrap_or_default();
        format!("https://devnet.helius-rpc.com/?api-key={key}")
    })
}

/// The state tree the deployment created. There is no discovery, so it must be
/// supplied in `ZOLANA_TREE`.
fn tree_from_env() -> Result<Pubkey> {
    let s = std::env::var("ZOLANA_TREE").map_err(|_| anyhow!("ZOLANA_TREE must be set"))?;
    s.parse::<Pubkey>()
        .map_err(|e| anyhow!("parse ZOLANA_TREE {s}: {e}"))
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
pub fn fund_key(client: &mut Client, to: &Pubkey, lamports: u64) -> Result<Signature> {
    let payer = client.payer.insecure_clone();
    let ix = system_transfer_ix(&payer.pubkey(), to, lamports);
    send(&mut client.rpc, &[ix], &payer.pubkey(), &[&payer])
}

/// Connect to an existing devnet deployment: read the endpoints, payer, and tree
/// from the environment. Runs no validator, airdrop, or admin setup.
pub fn setup() -> Result<(Client, Localnet)> {
    let indexer_url =
        std::env::var("ZOLANA_INDEXER_URL").unwrap_or_else(|_| DEFAULT_INDEXER_URL.into());
    let prover_url = std::env::var("ZOLANA_PROVER_URL").unwrap_or_default();
    let indexer = ZolanaIndexer::new(indexer_url);
    // Attach the indexer so one rpc both fetches spend proofs and sends, which
    // the one-call Submit action (transfer, withdraw) needs.
    let rpc = SolanaRpc::new(rpc_url()).with_indexer(indexer.clone());
    let program_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    rpc.assert_executable(&program_id)?;

    Ok((
        Client {
            rpc,
            indexer,
            prover: ProverClient::new(prover_url),
            tree: tree_from_env()?,
            payer: load_payer()?,
        },
        Localnet {
            assets: AssetRegistry::default(),
            spls: Vec::new(),
        },
    ))
}

/// Create a private wallet: a keypair, a funded Solana key, and an empty wallet.
/// Register it so others can send to its Solana address privately.
pub fn setup_private_wallet(
    client: &mut Client,
    localnet: &Localnet,
) -> Result<(ShieldedKeypair, Keypair, Wallet)> {
    let keypair = ShieldedKeypair::new()?;
    let funding = Keypair::new();
    // The payer covers the fee key; registration is the only thing it pays for.
    fund_key(client, &funding.pubkey(), 20_000_000)?;
    let wallet = Wallet::new(keypair.clone(), localnet.assets.clone())?;
    // Publish the wallet's keys so transfers to its Solana address stay private
    // instead of becoming a public withdrawal. Skip it where the user-registry
    // program is not deployed; deposits and reads work without it.
    let registry_id = Address::new_from_array(user_registry_program_id().to_bytes());
    let registry_deployed = client
        .rpc
        .get_account(registry_id)?
        .map(|a| a.executable)
        .unwrap_or(false);
    if registry_deployed {
        zolana_client::ensure_registered(&client.rpc, &funding, &keypair)?;
    } else {
        eprintln!("note: user-registry program not deployed; skipping registration");
    }
    Ok((keypair, funding, wallet))
}

/// Register a throwaway SPL mint for private balances and transactions. The payer
/// signs the registration directly (the deployment allows permissionless SPL
/// interface creation). Idempotent within a run.
pub fn register_asset(client: &mut Client, localnet: &mut Localnet) -> Result<SplAsset> {
    if let Some(asset) = localnet.spls.first() {
        return Ok(*asset);
    }
    let payer = client.payer.insecure_clone();
    let asset_id = FIRST_SPL_ASSET_ID;

    let mint = create_mint(&client.rpc, &payer)?;

    // The asset counter is a one-time singleton; create it only if missing.
    let counter_addr = Address::new_from_array(pda::spl_asset_counter().to_bytes());
    if client.rpc.get_account(counter_addr)?.is_none() {
        let ix = CreateAssetCounter {
            authority: payer.pubkey(),
        }
        .instruction();
        send(&mut client.rpc, &[ix], &payer.pubkey(), &[&payer])?;
    }

    let ix = CreateSplInterface {
        authority: payer.pubkey(),
        mint,
    }
    .instruction();
    send(&mut client.rpc, &[ix], &payer.pubkey(), &[&payer])?;

    let user_token = create_token_account(&client.rpc, &payer, &mint, &payer.pubkey())?;
    localnet
        .assets
        .insert(asset_id, Address::new_from_array(mint.to_bytes()))
        .map_err(|e| anyhow!("register SPL asset: {e}"))?;

    let asset = SplAsset { mint, user_token };
    localnet.spls.push(asset);
    Ok(asset)
}

/// Move `amount` of `asset` into the private balance of `keypair` and refresh
/// `wallet`. Pass `Some` token account for an SPL deposit, or `None` for SOL.
fn deposit(
    client: &mut Client,
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
    let payer = client.payer.insecure_clone();
    let _signature = prepared.send(&client.rpc, &payer, client.tree, &payer)?;
    // Sync the private balance.
    sync_wallet(wallet, &client.indexer)?;
    Ok(())
}

/// Move `amount` of SOL into the private balance of `keypair`. This is shared
/// setup for the transfer, withdraw, and sync examples.
pub fn deposit_sol(
    client: &mut Client,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    amount: u64,
) -> Result<()> {
    deposit(client, keypair, wallet, SOL_MINT, amount, None)
}

/// Fund the token account, then move `amount` of an SPL asset into the private
/// balance of `keypair`. This is shared setup for the transfer and withdraw examples.
pub fn deposit_spl(
    client: &mut Client,
    keypair: &ShieldedKeypair,
    wallet: &mut Wallet,
    asset: &SplAsset,
    amount: u64,
) -> Result<()> {
    let payer = client.payer.insecure_clone();
    mint_to(&client.rpc, &payer, &asset.mint, &asset.user_token, amount)?;
    deposit(
        client,
        keypair,
        wallet,
        Address::new_from_array(asset.mint.to_bytes()),
        amount,
        Some(asset.user_token),
    )
}

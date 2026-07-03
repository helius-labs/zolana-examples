//! Shared setup for the client examples. `setup_localnet` spins up a local
//! validator, the Photon indexer, and the prover, and returns a [`Client`] and
//! a [`Localnet`].

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;
use zolana_client::{
    create_deposit, spawn_prover, sync_wallet, CreateDeposit, ProverClient, Rpc, SolanaRpc,
    ZolanaIndexer,
};
use zolana_interface::{
    instruction::{CreateAssetCounter, CreateProtocolConfig, CreateSplInterface, CreateTree},
    pda,
    state::tree_account_size,
    SHIELDED_POOL_PROGRAM_ID,
};
use zolana_keypair::ShieldedKeypair;
use zolana_test_utils::{
    smart_account::{self, execute_sync_ix, StandardSigners},
    spl::{create_mint, create_token_account, mint_to},
};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};
use zolana_user_registry_interface::user_registry_program_id;

const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_INDEXER_URL: &str = "http://127.0.0.1:8784";
const DEFAULT_PROVER_URL: &str = "http://127.0.0.1:3001";

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

/// Local admin state for the test setup.
pub struct Localnet {
    assets: AssetRegistry,
    authority: Keypair,
    protocol_settings: Pubkey,
    protocol_vault: Pubkey,
    spls: Vec<SplAsset>,
}

/// Start the prover and point it at the committed keys.
fn start_prover() -> Result<()> {
    std::env::set_var(
        "ZOLANA_PROVER_KEYS_DIR",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prover/server/proving-keys"
        ),
    );
    spawn_prover()?;
    Ok(())
}

/// Restart a fresh validator and Photon indexer through the zolana CLI.
fn restart_localnet() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let cli =
        std::env::var("ZOLANA_CLI_BIN").unwrap_or_else(|_| format!("{root}/target/debug/zolana"));
    let program_id =
        std::env::var("SHIELDED_POOL_PROGRAM_ID").expect("SHIELDED_POOL_PROGRAM_ID must be set");
    let rpc_port = std::env::var("ZOLANA_LOCALNET_RPC_PORT").unwrap_or_else(|_| "8899".to_string());
    let photon_port =
        std::env::var("ZOLANA_LOCALNET_PHOTON_PORT").unwrap_or_else(|_| "8784".to_string());
    let program_so = format!("{root}/target/deploy/shielded_pool_program.so");

    let user_registry_id = user_registry_program_id().to_string();
    let user_registry_so = format!("{root}/target/deploy/zolana_user_registry.so");

    let smart_account_id = smart_account::SMART_ACCOUNT_PROGRAM_ID.to_string();
    let smart_account_so = format!("{root}/target/deploy/squads_smart_account_program.so");

    let account_dir = "/tmp/zolana-rust-client-example-accounts";
    smart_account::write_program_config_fixture(account_dir);

    let status = std::process::Command::new(&cli)
        .current_dir(root)
        .args([
            "test-validator",
            "--no-use-surfpool",
            "--with-photon",
            "--skip-prover",
            "--rpc-port",
            &rpc_port,
            "--photon-port",
            &photon_port,
            "--ledger",
            "/tmp/zolana-rust-client-example-ledger",
            "--sbf-program",
            &program_id,
            &program_so,
            "--sbf-program",
            &user_registry_id,
            &user_registry_so,
            "--sbf-program",
            &smart_account_id,
            &smart_account_so,
            "--account-dir",
            account_dir,
        ])
        .status()
        .expect("run zolana test-validator");
    assert!(status.success(), "zolana test-validator restart failed");
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

/// Boot a fresh localnet and create the protocol config and the state tree.
pub fn setup_localnet() -> Result<(Client, Localnet)> {
    let prover = std::thread::spawn(start_prover);
    restart_localnet();
    prover.join().expect("prover startup thread panicked")?;

    let rpc_url = std::env::var("ZOLANA_LOCALNET_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.into());
    let indexer_url =
        std::env::var("ZOLANA_INDEXER_URL").unwrap_or_else(|_| DEFAULT_INDEXER_URL.into());
    let prover_url =
        std::env::var("ZOLANA_PROVER_URL").unwrap_or_else(|_| DEFAULT_PROVER_URL.into());
    let indexer = ZolanaIndexer::new(indexer_url);
    // Attach the indexer so one rpc both fetches spend proofs and sends, which
    // the one-call Submit action (transfer, withdraw) needs.
    let mut rpc = SolanaRpc::new(rpc_url).with_indexer(indexer.clone());
    let program_id = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    rpc.assert_executable(&program_id)?;

    let payer = Keypair::new();
    let authority = Keypair::new();
    let forester_key = Keypair::new();
    let merge_key = Keypair::new();
    let tree_key = Keypair::new();
    let zone_key = Keypair::new();
    rpc.airdrop(&payer.pubkey(), 100_000_000_000)?;
    rpc.airdrop(&authority.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&forester_key.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&merge_key.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&tree_key.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&zone_key.pubkey(), 1_000_000_000)?;

    let accounts = smart_account::standard_accounts();
    for ix in accounts.create_ixs(
        &payer.pubkey(),
        StandardSigners {
            protocol: authority.pubkey(),
            forester: forester_key.pubkey(),
            merge: merge_key.pubkey(),
            tree: tree_key.pubkey(),
            zone: zone_key.pubkey(),
        },
    ) {
        send(&mut rpc, &[ix], &payer.pubkey(), &[&payer])?;
    }

    // The protocol vault pays for its own setup.
    rpc.airdrop(&accounts.protocol_vault, 5_000_000_000)?;

    let create_config_ix = CreateProtocolConfig {
        authority: accounts.protocol_vault,
        protocol_authority: accounts.protocol_vault.to_bytes().into(),
        tree_creation_authority: accounts.tree_vault.to_bytes().into(),
        tree_creation_is_permissionless: false,
        forester_authority: accounts.forester_vault.to_bytes().into(),
        zone_creation_authority: accounts.zone_vault.to_bytes().into(),
        zone_creation_is_permissionless: false,
        spl_interface_creation_is_permissionless: false,
    }
    .instruction();
    let create_config_sync = execute_sync_ix(
        &accounts.protocol_settings,
        0,
        &[authority.pubkey()],
        &[create_config_ix],
    );
    send(
        &mut rpc,
        &[create_config_sync],
        &payer.pubkey(),
        &[&payer, &authority],
    )?;

    let tree = Keypair::new();
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(tree_account_size())
        .map_err(|e| anyhow!("{e}"))?;
    let alloc_ix = zolana_program_test::system_create_account_ix(
        &payer.pubkey(),
        &tree.pubkey(),
        rent,
        tree_account_size() as u64,
        &pda::shielded_pool_program_id(),
    );
    let create_tree_ix = CreateTree {
        authority: accounts.tree_vault,
        tree: tree.pubkey(),
        owner: accounts.tree_vault,
    }
    .instruction();
    let create_tree_sync = execute_sync_ix(
        &accounts.tree_settings,
        0,
        &[tree_key.pubkey()],
        &[create_tree_ix],
    );
    send(
        &mut rpc,
        &[alloc_ix, create_tree_sync],
        &payer.pubkey(),
        &[&payer, &tree, &tree_key],
    )?;

    Ok((
        Client {
            rpc,
            indexer,
            prover: ProverClient::new(prover_url),
            tree: tree.pubkey(),
            payer,
        },
        Localnet {
            assets: AssetRegistry::default(),
            authority,
            protocol_settings: accounts.protocol_settings,
            protocol_vault: accounts.protocol_vault,
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
    client.rpc.airdrop(&funding.pubkey(), 1_000_000_000)?;
    let wallet = Wallet::new(keypair.clone(), localnet.assets.clone())?;
    // Publish the wallet's keys so transfers to its Solana address stay private
    // instead of becoming a public withdrawal.
    zolana_client::ensure_registered(&client.rpc, &funding, &keypair)?;
    Ok((keypair, funding, wallet))
}

/// Register an SPL mint for private balances and transactions. This is idempotent.
pub fn register_asset(client: &mut Client, localnet: &mut Localnet) -> Result<SplAsset> {
    if let Some(asset) = localnet.spls.first() {
        return Ok(*asset);
    }
    let payer = client.payer.insecure_clone();
    let authority = localnet.authority.insecure_clone();
    let asset_id = FIRST_SPL_ASSET_ID;

    let mint = create_mint(&client.rpc, &payer)?;

    // Registering an asset is an admin action.
    let counter_addr = Address::new_from_array(pda::spl_asset_counter().to_bytes());
    if client.rpc.get_account(counter_addr)?.is_none() {
        let ix = CreateAssetCounter {
            authority: localnet.protocol_vault,
        }
        .instruction();
        let sync_ix = execute_sync_ix(&localnet.protocol_settings, 0, &[authority.pubkey()], &[ix]);
        send(
            &mut client.rpc,
            &[sync_ix],
            &payer.pubkey(),
            &[&payer, &authority],
        )?;
    }

    let ix = CreateSplInterface {
        authority: localnet.protocol_vault,
        mint,
    }
    .instruction();
    let sync_ix = execute_sync_ix(&localnet.protocol_settings, 0, &[authority.pubkey()], &[ix]);
    send(
        &mut client.rpc,
        &[sync_ix],
        &payer.pubkey(),
        &[&payer, &authority],
    )?;

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

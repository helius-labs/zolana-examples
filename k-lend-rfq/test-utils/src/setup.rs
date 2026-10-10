use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, Result};
use k_lend_market_maker::{
    ConcurrencyConfig, ConnectionConfig, MarketMaker, MarketMakerConfig, QuoteConfig, TokenConfig,
};
use k_lend_rfq_sdk::{
    kvault::{global_config_address, KLEND_PROGRAM_ID, KVAULT_PROGRAM_ID},
    pair::Pair,
};
use serde_json::json;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_rpc_client_api::request::RpcRequest;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc};
use zolana_interface::{
    pda::{self, spl_token_program_id},
    state::SplAssetRegistry,
};
use zolana_program::instruction::CreateSplInterface;
use zolana_program_test::{
    fixture,
    instructions::system_create_account_ix,
    localnet::{FixtureLocalnet, LocalnetPaths, LocalnetPorts},
};
use zolana_test_utils::wallet::{create_associated_token_account, Deposit, DepositParams};
use zolana_transaction::AssetRegistry;

use crate::{
    chain::{blocking, send, wait_for_account},
    kvault::{self, InitVault},
    mainnet,
    user::User,
    wallet::TestWallet,
};

/// Market maker quote fee, in basis points of the swap amount.
pub const FEE_BPS: u64 = 30;
/// Default collateral each user shields at setup, in collateral base units,
/// split evenly over `USER_UTXOS` deposits.
pub const USER_SHIELD_COLLATERAL: u64 = 100_000_000;
const USER_UTXOS: u64 = 2;
const MAKER_ACTOR: u8 = 0;
const FIRST_USER_ACTOR: u8 = 1;
/// Collateral transferred to the market maker's public token account at
/// setup, in collateral base units.
pub const MARKET_MAKER_PUBLIC_COLLATERAL: u64 = 500_000_000;
/// Tokens of a snapshot vault's mint written to the payer's account at
/// setup, in base units; covers the users' and the maker's funding.
const SNAPSHOT_PAYER_TOKENS: u64 = 1_000_000_000_000;
/// The SPL Token program's `Transfer` instruction tag
/// (`TokenInstruction::Transfer`, variant 3), followed by the amount as a
/// little-endian u64.
const SPL_TRANSFER_TAG: u8 = 3;
/// How long setup waits for the pair's vault to be readable before it
/// starts the market maker.
pub const VAULT_VISIBLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Collateral of the maker's seed vault deposit in most tests, in collateral
/// base units; a test that needs another seed keeps its own constant.
pub const SEED_DEPOSIT: u64 = 200_000_000;
/// Minimum UTXO value of the inventory profiles tests configure, in base
/// units.
pub const MIN_UTXO_VALUE: u64 = 1_000_000;
/// How often a test polls the market maker for a condition.
pub const POLL: Duration = Duration::from_millis(500);
/// Longest a test waits for an automatic rebalance to land.
pub const REBALANCE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a test waits after a rebalance is indexed before it syncs the
/// market maker.
pub const SETTLE_GRACE: Duration = Duration::from_secs(3);

pub struct TestEnv {
    pub localnet: FixtureLocalnet,
    pub user: User,
    pub users: Vec<User>,
    pub market_maker: MarketMaker,
    /// The wallet behind `market_maker`, for tests that build or sign maker
    /// transfers by hand.
    pub market_maker_wallet: TestWallet,
    pub collateral_mint: Address,
    pub pair: Pair,
}

#[derive(Clone, Debug)]
pub struct SetupConfig {
    /// Test index; selects the localnet ports so tests run in parallel.
    ///
    /// The ports come from zolana's `LocalnetPorts::for_test`: RPC
    /// `8899 + 1000 * test` and Photon `8784 + 1000 * test`, plus
    /// `ZOLANA_PORT_OFFSET`. Indexes from 24 up put them inside Linux's
    /// ephemeral range (32768-60999), where an outbound socket of another
    /// process can hold the port and the localnet then fails to bind. The
    /// formula is zolana's, so it is kept; a bind failure on a high index
    /// is that collision, and a rerun or another `ZOLANA_PORT_OFFSET`
    /// clears it.
    pub test: u16,
    /// Users funded in addition to the first one.
    pub extra_users: u8,
    pub concurrency: ConcurrencyConfig,
    /// Range and profile of the pair's collateral mint.
    pub collateral: TokenConfig,
    /// Range and profile of the pair's shares mint.
    pub shares: TokenConfig,
    /// How long a market maker quote stays fillable.
    pub order_ttl: Duration,
    /// Collateral each user shields at setup, in collateral base units; must
    /// split evenly over `USER_UTXOS` deposits.
    pub user_collateral: u64,
    /// The vault the pair trades: a new uninvested one, or a snapshot.
    pub vault: VaultSource,
}

/// Where the pair's vault comes from.
#[derive(Clone, Debug, Default)]
pub enum VaultSource {
    /// A new vault over the fixture SPL mint, created at setup.
    #[default]
    Local,
    /// `vault` written from `accounts` (see `mainnet::snapshot_vault`); the
    /// pair's collateral is the vault's token mint, which setup registers in
    /// the shielded pool and funds the payer with.
    Snapshot {
        vault: Address,
        accounts: Vec<(Address, Account)>,
    },
}

impl SetupConfig {
    pub fn new(test: u16) -> Self {
        Self {
            test,
            extra_users: 0,
            concurrency: ConcurrencyConfig::default(),
            collateral: TokenConfig::default(),
            shares: TokenConfig::default(),
            order_ttl: QuoteConfig::default().order_ttl,
            user_collateral: USER_SHIELD_COLLATERAL,
            vault: VaultSource::Local,
        }
    }
}

fn token_transfer_ix(
    source: &Address,
    destination: &Address,
    authority: &Address,
    amount: u64,
) -> Instruction {
    let mut data = vec![SPL_TRANSFER_TAG];
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: spl_token_program_id(),
        accounts: vec![
            AccountMeta::new(*source, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

/// Creates and initializes a new uninvested vault over `collateral_mint`,
/// paid and administered by `payer`, and returns its pair. Registers no mint
/// in the shielded pool; the kVault global config must already exist.
pub fn create_vault(rpc: &SolanaRpc, payer: &Keypair, collateral_mint: Address) -> Result<Pair> {
    let vault_keypair = Keypair::new();
    let pair = Pair::new(vault_keypair.pubkey(), collateral_mint);
    let rent = rpc.get_minimum_balance_for_rent_exemption(kvault::VAULT_STATE_SIZE)?;
    let create = system_create_account_ix(
        &payer.pubkey(),
        &pair.vault,
        rent,
        kvault::VAULT_STATE_SIZE as u64,
        &KVAULT_PROGRAM_ID,
    );
    let init = InitVault {
        admin: payer.pubkey(),
        admin_token_account: fixture::payer_token_account(),
        pair,
    }
    .instruction();
    send(rpc, &[create, init], payer, &[payer, &vault_keypair])?;
    Ok(pair)
}

/// Registers `mint` in the shielded pool and returns its asset id.
fn register_spl_mint(rpc: &SolanaRpc, payer: &Keypair, mint: Address) -> Result<u64> {
    let ix = CreateSplInterface {
        authority: payer.pubkey(),
        mint,
        token_program: spl_token_program_id(),
    }
    .instruction();
    send(rpc, &[ix], payer, &[payer])?;
    let registry = pda::spl_asset_registry(&mint);
    let data = rpc
        .get_account(registry)?
        .ok_or_else(|| anyhow!("mint registry {registry} missing"))?
        .data;
    Ok(SplAssetRegistry::from_account_bytes(&data)
        .map_err(|e| anyhow!("mint registry {registry}: {e:?}"))?
        .asset_id)
}

/// The pair's collateral, its asset id and the payer's token account users
/// and the maker are funded from.
struct Collateral {
    mint: Address,
    asset_id: u64,
    payer_account: Address,
}

/// A new uninvested vault over the fixture SPL mint, with the kVault global
/// config written for it.
fn local_vault(rpc: &SolanaRpc, payer: &Keypair) -> Result<(Pair, Collateral)> {
    set_account(
        rpc,
        &global_config_address(),
        &kvault::global_config_account(&payer.pubkey())?,
    )?;
    let mint = fixture::spl_mint();
    let pair = create_vault(rpc, payer, mint)?;
    Ok((
        pair,
        Collateral {
            mint,
            asset_id: fixture::SPL_ASSET_ID,
            payer_account: fixture::payer_token_account(),
        },
    ))
}

/// Writes the snapshot `accounts` of `vault`, registers its token mint in the
/// shielded pool and writes the payer an account of that mint holding
/// `SNAPSHOT_PAYER_TOKENS`.
fn snapshot_vault(
    rpc: &SolanaRpc,
    payer: &Keypair,
    vault: &Address,
    accounts: &[(Address, Account)],
) -> Result<(Pair, Collateral)> {
    for (address, account) in accounts {
        set_account(rpc, address, account)?;
    }
    let vault_data = &accounts
        .iter()
        .find(|(address, _)| address == vault)
        .ok_or_else(|| anyhow!("snapshot lacks vault {vault}"))?
        .1
        .data;
    let view = k_lend_rfq_sdk::kvault::vault_state(vault_data)?;
    let pair = Pair::new(*vault, view.token_mint);
    anyhow::ensure!(
        (pair.token_vault, pair.shares_mint, pair.authority)
            == (
                view.token_vault,
                view.shares_mint,
                view.base_vault_authority
            ),
        "vault {vault} accounts are not the kVault PDAs"
    );
    let asset_id = register_spl_mint(rpc, payer, view.token_mint)?;
    let payer_account = pda::associated_token_address(&payer.pubkey(), &view.token_mint);
    set_account(
        rpc,
        &payer_account,
        &mainnet::token_account(&view.token_mint, &payer.pubkey(), SNAPSHOT_PAYER_TOKENS)?,
    )?;
    Ok((
        pair,
        Collateral {
            mint: view.token_mint,
            asset_id,
            payer_account,
        },
    ))
}

struct Booted {
    localnet: FixtureLocalnet,
    users: Vec<TestWallet>,
    maker: TestWallet,
    collateral_mint: Address,
    pair: Pair,
}

pub async fn setup(test: u16) -> Result<TestEnv> {
    setup_with(SetupConfig::new(test)).await
}

pub async fn setup_with(config: SetupConfig) -> Result<TestEnv> {
    let ports = LocalnetPorts::for_test(config.test)?;
    let Booted {
        localnet,
        users,
        maker,
        collateral_mint,
        pair,
    } = blocking(|| boot(ports, &config))?;
    // Surfpool can lag in exposing a just-written account, and the maker's start reads the vault.
    blocking(|| wait_for_account(localnet.client.rpc(), &pair.vault, VAULT_VISIBLE_TIMEOUT))?;
    let market_maker = MarketMaker::start(MarketMakerConfig {
        connection: ConnectionConfig {
            rpc_url: ports.rpc_url(),
            photon_url: ports.photon_url(),
            prover_url: None,
            tree: localnet.tree,
            tree_id: localnet.tree_id,
        },
        identity: maker.identity_config(),
        pairs: vec![pair],
        tokens: vec![
            (pair.token_mint, config.collateral),
            (pair.shares_mint, config.shares),
        ],
        concurrency: config.concurrency,
        quotes: QuoteConfig {
            fee_bps: FEE_BPS,
            order_ttl: config.order_ttl,
        },
    })
    .await?;
    let mut users = users
        .into_iter()
        .map(|wallet| User::new(wallet, localnet.tree, localnet.tree_id))
        .collect::<Vec<_>>()
        .into_iter();
    let user = users
        .next()
        .ok_or_else(|| anyhow!("setup funded no user"))?;
    Ok(TestEnv {
        localnet,
        user,
        users: users.collect(),
        market_maker,
        market_maker_wallet: maker,
        collateral_mint,
        pair,
    })
}

/// `file` under this workspace's `target/deploy`, where
/// `scripts/dump-kamino.sh` writes the Kamino programs.
fn deploy_path(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/deploy")
        .join(file)
}

/// `ZOLANA_PROVER_KEYS_DIR`, else `~/.config/zolana/proving-keys`, the
/// directory `zolana test-env` keeps the proving keys in.
fn proving_keys_dir() -> Result<PathBuf> {
    if let Ok(dir) = env::var("ZOLANA_PROVER_KEYS_DIR") {
        return Ok(dir.into());
    }
    let home = env::var("HOME").map_err(|_| anyhow!("HOME is not set"))?;
    Ok(Path::new(&home).join(".config/zolana/proving-keys"))
}

/// Write `account` at `address` through surfpool's `surfnet_setAccount`
/// cheatcode. The kVault global config is created by an admin instruction
/// that only the program's upgrade authority may send, and a snapshot
/// vault's accounts come from another cluster, so the test writes both.
/// `FixtureLocalnet::start_with_accounts` replaces this after `v0.4.0-alpha`.
fn set_account(rpc: &SolanaRpc, address: &Address, account: &Account) -> Result<()> {
    rpc.client().send::<serde_json::Value>(
        RpcRequest::Custom {
            method: "surfnet_setAccount",
        },
        json!([
            address.to_string(),
            {
                "lamports": account.lamports,
                "data": account.data.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                "owner": account.owner.to_string(),
                "executable": account.executable,
                "rentEpoch": account.rent_epoch,
            },
        ]),
    )?;
    Ok(())
}

fn boot(ports: LocalnetPorts, config: &SetupConfig) -> Result<Booted> {
    let payer = fixture::payer();
    let localnet = FixtureLocalnet::start(
        "zolana-k-lend",
        ports,
        vec![
            (KVAULT_PROGRAM_ID, deploy_path("kamino_vault.so")),
            (KLEND_PROGRAM_ID, deploy_path("kamino_lending.so")),
        ],
        &LocalnetPaths {
            proving_keys_dir: proving_keys_dir()?,
            ..LocalnetPaths::workspace()
        },
    )?;
    let rpc = localnet.client.rpc();
    let (pair, collateral) = match &config.vault {
        VaultSource::Local => local_vault(rpc, &payer)?,
        VaultSource::Snapshot { vault, accounts } => snapshot_vault(rpc, &payer, vault, accounts)?,
    };
    let collateral_mint = collateral.mint;
    let share_asset_id = register_spl_mint(rpc, &payer, pair.shares_mint)?;

    let mut assets = AssetRegistry::default();
    assets.insert(collateral.asset_id, collateral_mint)?;
    assets.insert(share_asset_id, pair.shares_mint)?;
    let market_maker = TestWallet::new(MAKER_ACTOR, &assets)?;

    for mint in [collateral_mint, pair.shares_mint] {
        create_associated_token_account(rpc, &payer, &market_maker.address(), &mint)?;
    }
    send(
        rpc,
        &[token_transfer_ix(
            &collateral.payer_account,
            &pda::associated_token_address(&market_maker.address(), &collateral_mint),
            &payer.pubkey(),
            MARKET_MAKER_PUBLIC_COLLATERAL,
        )],
        &payer,
        &[&payer],
    )?;

    let last_user_actor = FIRST_USER_ACTOR
        .checked_add(config.extra_users)
        .ok_or_else(|| {
            anyhow!(
                "{} extra users overflow the u8 actor id",
                config.extra_users
            )
        })?;
    anyhow::ensure!(
        config.user_collateral.is_multiple_of(USER_UTXOS),
        "user collateral {} does not split evenly over {USER_UTXOS} deposits",
        config.user_collateral
    );
    let user_utxo_amount = config.user_collateral / USER_UTXOS;
    let mut users = Vec::new();
    for actor in FIRST_USER_ACTOR..=last_user_actor {
        let mut user = TestWallet::new(actor, &assets)?;
        for _ in 0..USER_UTXOS {
            let user_deposit = Deposit::new(DepositParams {
                recipient: &user.keys().address()?,
                asset: collateral_mint,
                amount: user_utxo_amount,
                spl_token_account: Some(collateral.payer_account),
                spl_token_program: Some(spl_token_program_id()),
                memo: None,
            })?
            .send(rpc, &payer, localnet.tree, &payer)?;
            localnet
                .client
                .confirm_private_transaction_sync(user_deposit)
                .map_err(|e| anyhow!("index deposit of user {actor}: {e:?}"))?;
        }
        user.sync(localnet.client.indexer())?;
        users.push(user);
    }

    Ok(Booted {
        localnet,
        users,
        maker: market_maker,
        collateral_mint,
        pair,
    })
}

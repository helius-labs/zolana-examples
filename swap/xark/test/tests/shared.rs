use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_signature::Signature;
use solana_signer::Signer;
use spr_client::{ComputeBudgetConfig, Rpc, SolanaRpc};
use spr_keypair::{ShieldedKeypair, SigningKey};
use spr_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPaths, LocalnetPorts},
};
use spr_test_utils::test_validator_asserts::wait_for_indexed_utxo;
use spr_test_utils::wallet::{sync_wallet, Deposit, DepositParams, Filter, Wallet};
use spr_transaction::{AssetRegistry, WalletUtxo, SOL_MINT};
use spr_user_registry_interface::user_registry_program_id;

use std::{
    env,
    path::{Path, PathBuf},
};

// The whole per-transaction budget: a swap verifies an SPP proof and its own.
const TRANSACT_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

// SPL the maker shields into the order UTXO (source), and SOL the taker pays (destination).
pub const MAKER_SHIELD_SPL: u64 = 1_000_000_000;
pub const SOURCE_AMOUNT: u64 = 400_000_000;
pub const DESTINATION_AMOUNT: u64 = 250_000_000;

// Each actor is one ed25519 identity: the wallet's signing key doubles as the
// Solana fee payer (`to_solana_keypair`), and the wallet holds the asset
// registry and the synced spendable notes.
pub struct TestEnv {
    /// The localnet with its client and default tree. Dropping it stops the
    /// validator, so it lives as long as the test.
    pub localnet: FixtureLocalnet,
    pub maker: TestWallet,
    /// The maker's SPL UTXO the make spends.
    pub maker_input: WalletUtxo,
    pub taker: TestWallet,
    pub spl_mint: Address,
}

pub struct TestWallet {
    pub wallet: Wallet,
    pub keypair: ShieldedKeypair,
}

impl std::ops::Deref for TestWallet {
    type Target = Wallet;
    fn deref(&self) -> &Self::Target {
        &self.wallet
    }
}

impl std::ops::DerefMut for TestWallet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.wallet
    }
}
/// `file` under this workspace's `target/deploy`, where `cargo build-sbf`
/// writes the program.
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

/// The CLI, prover, Photon, surfpool and pool program come from the
/// environment (`ZOLANA_CLI_BIN`, `PROVER_BIN`, `ZOLANA_PHOTON_BIN`,
/// `SURFPOOL_BIN`, `SHIELDED_POOL_PROGRAM_PATH`). The keys and the scratch
/// directory are set here: `LocalnetPaths::workspace` alone resolves them
/// inside the cargo git checkout of zolana.
fn localnet_paths() -> Result<LocalnetPaths> {
    Ok(LocalnetPaths {
        proving_keys_dir: proving_keys_dir()?,
        scratch_dir: Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/localnet"),
        ..LocalnetPaths::workspace()
    })
}

/// The user registry program, `USER_REGISTRY_PROGRAM_PATH`, which
/// `scripts/build-localnet.sh` downloads from the `user-registry-v1.0.0`
/// release.
fn user_registry_program_path() -> Result<PathBuf> {
    env::var("USER_REGISTRY_PROGRAM_PATH")
        .map(PathBuf::from)
        .map_err(|_| {
            anyhow!("USER_REGISTRY_PROGRAM_PATH is not set; see scripts/build-localnet.sh")
        })
}

/// Boot the localnet of test number `test` ([`LocalnetPorts::for_test`]); tests
/// running in parallel take distinct numbers.
pub fn setup(test: u16) -> Result<TestEnv> {
    let localnet = FixtureLocalnet::start(
        "zolana-swap",
        LocalnetPorts::for_test(test)?,
        vec![
            (swap_program::ID, deploy_path("swap_program.so")),
            (user_registry_program_id(), user_registry_program_path()?),
        ],
        &localnet_paths()?,
    )?;
    let payer = fixture::payer();

    let spl_mint = fixture::spl_mint();
    let mut assets = AssetRegistry::default();
    assets.insert(fixture::SPL_ASSET_ID, spl_mint)?;
    let spl_funding = fixture::payer_token_account();

    let maker_solana_keypair = fixture::actor(0);
    let maker_seed: [u8; 32] = maker_solana_keypair.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    let maker_shielded_keypair =
        ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&maker_seed))?;

    let taker_solana_keypair = fixture::actor(1);
    let taker_seed: [u8; 32] = taker_solana_keypair.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    let taker_shielded_keypair =
        ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&taker_seed))?;

    // Fund the actors: shield SPL to the maker and SOL to the taker.
    let maker_address = maker_shielded_keypair
        .shielded_address()
        .map_err(|e| anyhow!("maker address: {e:?}"))?;
    let taker_address = taker_shielded_keypair
        .shielded_address()
        .map_err(|e| anyhow!("taker address: {e:?}"))?;
    let maker_deposit = Deposit::new(DepositParams {
        recipient: &maker_address,
        asset: spl_mint,
        amount: MAKER_SHIELD_SPL,
        spl_token_account: Some(spl_funding),
        spl_token_program: Some(spr_interface::pda::spl_token_program_id()),
        memo: None,
    })?;
    let maker_view_tag = maker_deposit.view_tag();
    let maker_signature = maker_deposit.send(&localnet.client, &payer, localnet.tree, &payer)?;
    let taker_deposit = Deposit::new(DepositParams {
        recipient: &taker_address,
        asset: SOL_MINT,
        amount: DESTINATION_AMOUNT,
        spl_token_account: None,
        spl_token_program: Some(spr_interface::pda::spl_token_program_id()),
        memo: None,
    })?;
    let taker_view_tag = taker_deposit.view_tag();
    let taker_signature = taker_deposit.send(&localnet.client, &payer, localnet.tree, &payer)?;

    // Both deposits are wallet-owned, so discover them through the indexer
    // once they are indexed.
    wait_for_indexed_utxo(&localnet.client, maker_view_tag, maker_signature);
    wait_for_indexed_utxo(&localnet.client, taker_view_tag, taker_signature);
    let mut maker_wallet =
        Wallet::new(maker_address, assets.clone()).map_err(|e| anyhow!("maker wallet: {e:?}"))?;
    let mut taker_wallet =
        Wallet::new(taker_address, assets.clone()).map_err(|e| anyhow!("taker wallet: {e:?}"))?;
    sync_wallet(&mut maker_wallet, &maker_shielded_keypair, &localnet.client)
        .map_err(|e| anyhow!("sync maker deposit: {e:?}"))?;
    sync_wallet(&mut taker_wallet, &taker_shielded_keypair, &localnet.client)
        .map_err(|e| anyhow!("sync taker deposit: {e:?}"))?;
    let maker_input = maker_wallet
        .balance(spl_mint, Some(Filter::MinAmount(MAKER_SHIELD_SPL)))?
        .utxos
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no maker SPL note of {MAKER_SHIELD_SPL}"))?;
    assert_eq!(
        (maker_input.utxo.asset.asset, maker_input.utxo.amount),
        (spl_mint, MAKER_SHIELD_SPL)
    );

    let env = TestEnv {
        localnet,
        maker: TestWallet {
            wallet: maker_wallet,
            keypair: maker_shielded_keypair,
        },
        maker_input,
        taker: TestWallet {
            wallet: taker_wallet,
            keypair: taker_shielded_keypair,
        },
        spl_mint,
    };
    Ok(env)
}

// Submit a single (large) swap instruction as a transaction **v1** message:
// its 4096-byte limit is what holds a swap's account list and proof, which no
// longer fit a 1232-byte legacy packet. v1 has no address lookup table, and it
// carries the compute ceilings in the message header rather than in a
// compute-budget instruction. An unset ceiling means zero, not a default, so
// both are written. `payer` signs and pays.
pub fn send(rpc: &SolanaRpc, payer: &dyn Signer, ix: Instruction) -> Result<Signature> {
    Ok(rpc.create_and_send_transaction(
        std::slice::from_ref(&ix),
        payer.pubkey(),
        &[payer],
        ComputeBudgetConfig::new(TRANSACT_COMPUTE_UNIT_LIMIT),
    )?)
}

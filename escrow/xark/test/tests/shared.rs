use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;
use spr_client::{ComputeBudgetConfig, HistoryFilters, Rpc};
use spr_interface::pda::spl_token_program_id;
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPaths, LocalnetPorts},
};
use spr_test_utils::{
    test_validator_asserts::wait_for_indexed_utxo,
    wallet::{sync_wallet, Deposit, DepositParams, Wallet},
};
use spr_transaction::{AssetRegistry, ShieldedTransaction, WalletUtxo, SOL_MINT};
use timelock_escrow_sdk::escrow_authority;

// The whole per-transaction budget: the escrow forwards an SPP transact.
const TRANSACT_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

pub const SHIELD_AMOUNT: u64 = 500_000_000;
pub const LOCK_AMOUNT: u64 = 300_000_000;

// The committed unlock timestamp is already in the past, so the withdraw in
// these tests always succeeds immediately: the timelock escrow program
// requires `now > unlock_timestamp`. The SPP relayer deadline of the withdraw
// transact is a separate field, left at its default (no deadline).
pub const UNLOCK_TIMESTAMP: u64 = 1_000_000;

/// One party: a funded Solana keypair and the shielded keypair derived from
/// it.
pub struct Actor {
    pub solana: Keypair,
    pub shielded: ShieldedKeypair,
}

impl Actor {
    pub fn new(index: u8) -> Result<Self> {
        let solana = fixture::actor(index);
        let shielded = ShieldedKeypair::from_keypair(&solana)?;
        Ok(Self { solana, shielded })
    }

    pub fn address(&self) -> Result<ShieldedAddress> {
        Ok(self.shielded.shielded_address()?)
    }
}

// The creator is the only actor: one ed25519 identity whose Solana key pays
// the transactions and signs the withdraw.
pub struct TestEnv {
    /// The localnet with its client and default tree. Dropping it stops the
    /// validator, so it lives as long as the test.
    pub localnet: FixtureLocalnet,
    pub creator: Actor,
    /// The shielded balance the escrow locks from, owned by the creator.
    pub creator_input: WalletUtxo,
    /// The creator's wallet, synced after the deposit.
    pub wallet: Wallet,
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

/// Boot the localnet of test number `test` ([`LocalnetPorts::for_test`]); tests
/// running in parallel take distinct numbers. Then shield `SHIELD_AMOUNT` SOL
/// to the creator, the balance the escrow locks from.
pub fn setup(test: u16) -> Result<TestEnv> {
    let localnet = FixtureLocalnet::start(
        "timelock-escrow-xark",
        LocalnetPorts::for_test(test)?,
        vec![(
            timelock_escrow_program::ID,
            deploy_path("timelock_escrow_program.so"),
        )],
        &localnet_paths()?,
    )?;
    let payer = fixture::payer();
    let creator = Actor::new(0)?;

    // SOL only: asset id 1 is a built-in AssetRegistry::default() entry, no
    // SPL registration needed.
    let creator_address = creator.address()?;
    let deposit = Deposit::new(DepositParams {
        recipient: &creator_address,
        asset: SOL_MINT,
        amount: SHIELD_AMOUNT,
        spl_token_account: None,
        spl_token_program: Some(spl_token_program_id()),
        memo: None,
    })?;
    let view_tag = deposit.view_tag();
    let signature = deposit.send(&localnet.client, &payer, localnet.tree, &payer)?;
    wait_for_indexed_utxo(&localnet.client, view_tag, signature);

    // The creator's wallet reads the deposit back from the indexer.
    let mut wallet = Wallet::new(creator_address, AssetRegistry::default())
        .map_err(|e| anyhow!("creator wallet: {e:?}"))?;
    sync_wallet(&mut wallet, &creator.shielded, localnet.client.indexer())
        .map_err(|e| anyhow!("sync creator after deposit: {e:?}"))?;
    let balance = wallet
        .balance(SOL_MINT, None)
        .map_err(|e| anyhow!("creator balance: {e:?}"))?;
    let [creator_input]: [WalletUtxo; 1] = balance
        .utxos
        .try_into()
        .map_err(|utxos: Vec<WalletUtxo>| anyhow!("expected one deposit, found {}", utxos.len()))?;
    assert_eq!(
        (creator_input.utxo.asset.asset, creator_input.utxo.amount),
        (SOL_MINT, SHIELD_AMOUNT)
    );

    Ok(TestEnv {
        localnet,
        creator,
        creator_input,
        wallet,
    })
}

/// The shielded transactions tagged for the creator or for the outputs of
/// the creator's escrow-authority PDA.
pub fn creator_transactions(
    localnet: &FixtureLocalnet,
    creator: &Actor,
) -> Result<Vec<ShieldedTransaction>> {
    let tags = vec![
        creator.address()?.confidential_view_tag()?,
        escrow_authority(&creator.solana.pubkey()).confidential_view_tag(),
    ];
    Ok(localnet
        .client
        .get_shielded_transactions_by_tags(tags, None, Some(50), HistoryFilters::default(), None)?
        .data
        .into_iter()
        .flat_map(|item| item.events)
        .collect())
}

// Submit a single (large) instruction as a transaction **v1** message: its
// 4096-byte limit is what holds the escrow/withdraw account lists forwarding
// the SPP transact's tree accounts, which no longer fit a 1232-byte legacy
// packet. v1 has no address lookup table, and it carries the compute ceilings
// in the message header rather than in a compute-budget instruction. The first
// signer pays.
pub fn send(rpc: &impl Rpc, signers: &[&dyn Signer], ix: Instruction) -> Result<Signature> {
    let payer = signers
        .first()
        .ok_or_else(|| anyhow!("a transaction needs a fee payer"))?
        .pubkey();
    Ok(rpc.create_and_send_transaction(
        std::slice::from_ref(&ix),
        payer,
        signers,
        ComputeBudgetConfig::new(TRANSACT_COMPUTE_UNIT_LIMIT),
    )?)
}

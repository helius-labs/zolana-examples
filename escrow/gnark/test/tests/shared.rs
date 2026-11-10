use anyhow::{anyhow, Result};
use solana_instruction::Instruction;
use solana_signature::Signature;
use solana_signer::Signer;
use spr_client::{ComputeBudgetConfig, Rpc, SolanaRpc};
use spr_keypair::{ShieldedKeypair, SigningKey};
use spr_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPorts},
};
use spr_test_utils::test_validator_asserts::wait_for_indexed_utxo;
use spr_test_utils::wallet::{sync_wallet, Deposit, DepositParams, Wallet};
use spr_transaction::{utxo::SppProofInputUtxo, AssetRegistry, WalletUtxo, SOL_MINT};

#[path = "../../../../test-utils/localnet.rs"]
mod release_localnet;

// The whole per-transaction budget: the escrow forwards an SPP transact.
const TRANSACT_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

pub const SHIELD_AMOUNT: u64 = 500_000_000;
pub const LOCK_AMOUNT: u64 = 300_000_000;

// The committed unlock timestamp is already in the past, so the withdraw in
// these tests always succeeds immediately: the timelock escrow program
// requires `now > unlock_timestamp`.
pub const UNLOCK_TIMESTAMP: u64 = 1_000_000;

// The SPP relayer deadline on the withdraw transact must be in the future
// even when the escrow's own `unlock_timestamp` is already in the past (the
// two are unrelated fields; see timelock_escrow.md's Escrow Terms section).
pub const SPP_RELAYER_DEADLINE: u64 = 2_000_000_000;

// The creator is the only actor: one ed25519 identity whose signing key
// doubles as the Solana fee payer (`to_solana_keypair`), holding the asset
// registry and synced spendable notes.
pub struct TestEnv {
    /// The localnet with its client and default tree. Dropping it stops the
    /// validator, so it lives as long as the test.
    pub localnet: FixtureLocalnet,
    /// The creator, its wallet synced after the deposit.
    pub creator: TestWallet,
    /// The shielded balance the escrow locks from, owned by the creator.
    pub creator_input: SppProofInputUtxo,
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

/// Boot the localnet of test number `test` ([`LocalnetPorts::for_test`]); tests
/// running in parallel take distinct numbers.
pub fn setup(test: u16) -> Result<TestEnv> {
    let localnet = FixtureLocalnet::start(
        "timelock-escrow",
        LocalnetPorts::for_test(test)?,
        vec![(
            timelock_escrow_program::ID,
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target/deploy/timelock_escrow_program.so"),
        )],
        &release_localnet::paths(),
    )?;
    let payer = fixture::payer();

    // SOL only: asset id 1 is a built-in AssetRegistry::default() entry, no
    // SPL registration needed.
    let assets = AssetRegistry::default();

    let creator_solana_keypair = fixture::actor(0);
    let creator_seed: [u8; 32] = creator_solana_keypair.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    let creator_shielded_keypair =
        ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&creator_seed))?;

    let creator_address = creator_shielded_keypair
        .shielded_address()
        .map_err(|e| anyhow!("creator address: {e:?}"))?;

    // Shield the creator's balance, which the escrow locks from: the creator
    // owns it and signs its spend.
    let creator_deposit = Deposit::new(DepositParams {
        recipient: &creator_address,
        asset: SOL_MINT,
        amount: SHIELD_AMOUNT,
        spl_token_account: None,
        spl_token_program: Some(spr_interface::pda::spl_token_program_id()),
        memo: None,
    })?;
    let creator_view_tag = creator_deposit.view_tag();
    let creator_signature =
        creator_deposit.send(&localnet.client, &payer, localnet.tree, &payer)?;
    wait_for_indexed_utxo(&localnet.client, creator_view_tag, creator_signature);

    // The creator's wallet reads the deposit back from the indexer.
    let mut creator_wallet =
        Wallet::new(creator_address, assets).map_err(|e| anyhow!("creator wallet: {e:?}"))?;
    sync_wallet(
        &mut creator_wallet,
        &creator_shielded_keypair,
        localnet.client.indexer(),
    )
    .map_err(|e| anyhow!("sync creator after deposit: {e:?}"))?;
    let balance = creator_wallet
        .balance(SOL_MINT, None)
        .map_err(|e| anyhow!("creator balance: {e:?}"))?;
    let [creator_input]: [WalletUtxo; 1] = balance
        .utxos
        .try_into()
        .map_err(|utxos: Vec<WalletUtxo>| anyhow!("expected one deposit, found {}", utxos.len()))?;
    let creator_input: SppProofInputUtxo = creator_input.into();
    assert_eq!(
        (
            creator_input.utxo.owner,
            creator_input.utxo.asset.asset,
            creator_input.utxo.amount
        ),
        (creator_address.signing_pubkey, SOL_MINT, SHIELD_AMOUNT)
    );

    Ok(TestEnv {
        localnet,
        creator: TestWallet {
            wallet: creator_wallet,
            keypair: creator_shielded_keypair,
        },
        creator_input,
    })
}

// Submit a single (large) instruction as a transaction **v1** message: its
// 4096-byte limit is what holds the escrow/withdraw account lists forwarding
// the SPP transact's tree accounts, which no longer fit a 1232-byte legacy
// packet. v1 has no address lookup table, and it carries the compute ceilings
// in the message header rather than in a compute-budget instruction. An unset
// ceiling means zero, not a default, so both are written. `payer` signs and pays.
pub fn send(rpc: &SolanaRpc, payer: &dyn Signer, ix: Instruction) -> Result<Signature> {
    Ok(rpc.create_and_send_transaction(
        std::slice::from_ref(&ix),
        payer.pubkey(),
        &[payer],
        ComputeBudgetConfig::new(TRANSACT_COMPUTE_UNIT_LIMIT),
    )?)
}

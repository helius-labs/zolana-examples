use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use spr_client::{ComputeBudgetConfig, MerkleProof, Rpc, SolanaRpc, UtxoScan};
use spr_keypair::{ShieldedAddress, ShieldedKeypair, SigningKey};
use spr_program::instruction::{AssetDeposit, Deposit, DepositAsset, DepositSplAccounts};
use spr_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPorts},
};
use spr_transaction::{
    utxo::SppProofInputUtxo, AssetRegistry, SpendableDecryptionResult, WalletUtxo,
};
use spr_user_registry_interface::user_registry_program_id;

#[path = "../../../../test-utils/localnet.rs"]
mod release_localnet;

// The whole per-transaction budget: a swap verifies an SPP proof and its own.
const TRANSACT_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
const INDEXER_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

// SPL the maker shields into the order UTXO (source), and SOL the taker pays (destination).
pub const MAKER_SHIELD_SPL: u64 = 1_000_000_000;
pub const SOURCE_AMOUNT: u64 = 400_000_000;
pub const DESTINATION_AMOUNT: u64 = 250_000_000;

// Each actor is one ed25519 identity: the wallet's signing key doubles as the
// Solana fee payer, and the wallet holds the asset registry and the spendable
// notes its scan found after the deposits.
pub struct TestEnv {
    /// The localnet with its client and default tree. Dropping it stops the
    /// validator, so it lives as long as the test.
    pub localnet: FixtureLocalnet,
    pub maker: TestWallet,
    /// The maker's own SPL deposit, which funds the order.
    pub maker_input: SppProofInputUtxo,
    pub taker: TestWallet,
    pub spl_mint: Address,
}

pub struct TestWallet {
    pub keypair: ShieldedKeypair,
    pub registry: AssetRegistry,
    spendable: SpendableDecryptionResult,
}

impl TestWallet {
    /// Scan the indexer for the notes `keypair` can spend.
    fn scan(keypair: ShieldedKeypair, registry: AssetRegistry, client: &impl Rpc) -> Result<Self> {
        let spendable = UtxoScan::new(&keypair, &registry).fetch(client)?.spendable;
        Ok(Self {
            keypair,
            registry,
            spendable,
        })
    }

    /// The first spendable default-ring note of `mint` worth at least
    /// `min_amount`.
    pub fn spendable_utxo(&self, mint: Address, min_amount: u64) -> Option<WalletUtxo> {
        self.spendable
            .balances
            .get_balance(mint)?
            .utxos
            .iter()
            .find(|utxo| utxo.utxo.ring_program_id.is_none() && utxo.utxo.amount >= min_amount)
            .cloned()
    }
}
/// Boot the localnet of test number `test` ([`LocalnetPorts::for_test`]); tests
/// running in parallel take distinct numbers.
pub fn setup(test: u16) -> Result<TestEnv> {
    let localnet = FixtureLocalnet::start(
        "zolana-swap",
        LocalnetPorts::for_test(test)?,
        vec![
            (
                swap_program::ID,
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../program/target/deploy/swap_program.so"),
            ),
            (
                user_registry_program_id(),
                release_localnet::env_path("USER_REGISTRY_PROGRAM_PATH"),
            ),
        ],
        &release_localnet::paths(),
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

    // Fund the actors: shield the maker's SPL and the taker's SOL directly to
    // each. The maker spends its own SPL into the order.
    let maker_address = maker_shielded_keypair
        .shielded_address()
        .map_err(|e| anyhow!("maker address: {e:?}"))?;
    let maker_signature = deposit(
        &localnet,
        &payer,
        &maker_address,
        DepositAsset::Spl(DepositSplAccounts {
            mint: spl_mint,
            user_token: spl_funding,
            token_program: spr_interface::pda::spl_token_program_id(),
        }),
        MAKER_SHIELD_SPL,
    )?;
    let taker_address = taker_shielded_keypair
        .shielded_address()
        .map_err(|e| anyhow!("taker address: {e:?}"))?;
    let taker_signature = deposit(
        &localnet,
        &payer,
        &taker_address,
        DepositAsset::Sol,
        DESTINATION_AMOUNT,
    )?;

    // Both deposits are wallet-owned, so each wallet discovers its own
    // through the indexer once it is indexed.
    for signature in [maker_signature, taker_signature] {
        localnet
            .client
            .confirm_private_transaction_sync(signature)
            .map_err(|e| anyhow!("deposit {signature} indexed: {e:?}"))?;
    }
    let maker = TestWallet::scan(maker_shielded_keypair, assets.clone(), &localnet.client)
        .map_err(|e| anyhow!("scan maker deposit: {e:?}"))?;
    let maker_input = SppProofInputUtxo::from(
        maker
            .spendable_utxo(spl_mint, MAKER_SHIELD_SPL)
            .ok_or_else(|| anyhow!("the maker's SPL deposit is not spendable"))?,
    );
    let taker = TestWallet::scan(taker_shielded_keypair, assets, &localnet.client)
        .map_err(|e| anyhow!("scan taker deposit: {e:?}"))?;

    let env = TestEnv {
        localnet,
        maker,
        maker_input,
        taker,
        spl_mint,
    };

    // Guard the fixture: the maker-owned input the make flows spend must be
    // exactly the note the maker deposit just funded.
    assert_eq!(
        (
            env.maker_input.utxo.owner,
            env.maker_input.utxo.asset.asset,
            env.maker_input.utxo.amount
        ),
        (
            env.maker.keypair.signing_pubkey(),
            spl_mint,
            MAKER_SHIELD_SPL
        )
    );
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

/// Shield `amount` of `asset` from `payer` to `recipient` with a proofless
/// deposit. `payer` pays the fee and funds the deposit.
fn deposit(
    localnet: &FixtureLocalnet,
    payer: &dyn Signer,
    recipient: &ShieldedAddress,
    asset: DepositAsset,
    amount: u64,
) -> Result<Signature> {
    let ix = Deposit {
        tree: localnet.tree,
        depositor: payer.pubkey(),
        deposits: vec![AssetDeposit {
            asset,
            // Deposits carry the recipient's viewing key tag.
            view_tag: recipient.viewing_pubkey.u(),
            owner: recipient.owner_hash()?,
            amount,
            memo: None,
        }],
    }
    .instruction()?;
    Ok(localnet.client.rpc().create_and_send_transaction(
        std::slice::from_ref(&ix),
        payer.pubkey(),
        &[payer],
        ComputeBudgetConfig::for_instruction_count(1),
    )?)
}

/// Poll the indexer until it serves the inclusion proof of `leaf` in `tree`.
pub fn wait_for_merkle_proof(
    indexer: &impl Rpc,
    tree: Pubkey,
    leaf: [u8; 32],
) -> Result<MerkleProof> {
    let started = Instant::now();
    let mut last_error = None;
    while started.elapsed() < INDEXER_TIMEOUT {
        match indexer.get_merkle_proofs(tree, vec![leaf], None) {
            Ok(response) => {
                if let Some(proof) = response.proofs.into_iter().next() {
                    return Ok(proof);
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(anyhow!(
        "timed out waiting for an indexed merkle proof; last indexer error: {}",
        last_error.as_deref().unwrap_or("none")
    ))
}

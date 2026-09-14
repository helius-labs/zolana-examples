use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{
    AsyncProverClient, AsyncZolanaIndexer, ComputeBudgetConfig, ProverClient, Rpc, SolanaRpc,
    ZolanaClient, ZolanaIndexer,
};
use zolana_interface::{
    instruction::CreateProtocolConfig,
    state::{default_tree_fees, nullifier_tree_params},
    SHIELDED_POOL_PROGRAM_ID,
};
use zolana_keypair::{ShieldedKeypair, SigningKey};
use zolana_program_test::create_tree_instructions;
use zolana_test_utils::smart_account::{self, StandardSigners};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};
use zolana_wallet::{sync_wallet, Deposit, DepositParams};

mod services;
use services::LocalnetServices;

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

const TRANSACT_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

pub fn fetch_escrow_witnesses(
    indexer: &ZolanaIndexer,
    input_tree: Pubkey,
    inputs: &zolana_transaction::instructions::transact::SppProofInputs,
) -> Result<(
    Vec<zolana_client::SpendProof>,
    Vec<zolana_client::NonInclusionProof>,
)> {
    let commitments = inputs.input_utxo_hashes()?;
    let states = indexer
        .get_merkle_proofs(
            input_tree,
            commitments.iter().map(|c| c.utxo_hash).collect(),
            None,
        )?
        .proofs;
    let nullifiers = inputs
        .input_utxos
        .iter()
        .map(|input| input.nullifier())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let nullifiers = indexer
        .get_non_inclusion_proofs(input_tree, nullifiers, None)?
        .proofs;
    anyhow::ensure!(
        states.len() == commitments.len() && nullifiers.len() == inputs.input_utxos.len(),
        "indexer returned an incorrect witness count"
    );
    let mut states = states.into_iter();
    let mut real = Vec::new();
    let mut dummy = Vec::new();
    for (input, nullifier) in inputs.input_utxos.iter().zip(nullifiers) {
        if input.is_dummy() {
            dummy.push(nullifier);
        } else {
            real.push(zolana_client::SpendProof {
                state: states
                    .next()
                    .ok_or_else(|| anyhow!("missing inclusion proof"))?,
                nullifier,
            });
        }
    }
    Ok((real, dummy))
}

// The creator is the only actor: one ed25519 identity whose signing key
// doubles as the Solana fee payer (`to_solana_keypair`), holding the asset
// registry and synced spendable notes.
pub struct TestEnv {
    pub client: ZolanaClient<SolanaRpc>,
    pub tree: Pubkey,
    pub tree_id: u16,
    pub creator: TestWallet,
    pub _services: Option<LocalnetServices>,
}

pub struct TestWallet {
    pub wallet: Wallet,
    pub keypair: ShieldedKeypair,
    pub solana_keypair: Keypair,
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

pub fn setup() -> Result<TestEnv> {
    if std::env::var("ESCROW_TEST_NETWORK").as_deref() == Ok("devnet") {
        return setup_devnet();
    }

    let services = LocalnetServices::start()?;
    let mut rpc = SolanaRpc::new(services.rpc_url.clone());
    let indexer_url = services.indexer_url.clone();
    let indexer = ZolanaIndexer::new(indexer_url.clone());

    let spp_program = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    rpc.assert_executable(&spp_program)?;
    let escrow_program = Pubkey::new_from_array(*timelock_escrow_program::ID.as_array());
    rpc.assert_executable(&escrow_program)?;

    let payer = Keypair::new();
    let authority = Keypair::new();
    let forester_authority = Keypair::new();
    let merge_authority = Keypair::new();
    let tree_creation_authority = Keypair::new();
    let ring_creation_authority = Keypair::new();
    rpc.airdrop(&payer.pubkey(), 100_000_000_000)?;
    rpc.airdrop(&authority.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&forester_authority.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&merge_authority.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&tree_creation_authority.pubkey(), 1_000_000_000)?;
    rpc.airdrop(&ring_creation_authority.pubkey(), 1_000_000_000)?;

    let payer_address = payer.pubkey();

    let accounts = smart_account::standard_accounts();
    for ix in accounts.create_ixs(
        &payer.pubkey(),
        StandardSigners {
            protocol: authority.pubkey(),
            forester: forester_authority.pubkey(),
            merge: merge_authority.pubkey(),
            tree: tree_creation_authority.pubkey(),
            ring: ring_creation_authority.pubkey(),
        },
    ) {
        rpc.create_and_send_transaction(
            &[ix],
            payer_address,
            &[&payer],
            ComputeBudgetConfig::for_instruction_count(1),
        )?;
    }

    rpc.airdrop(&accounts.protocol_vault, 5_000_000_000)?;

    let create_config_ix = CreateProtocolConfig {
        fee_payer: payer.pubkey(),
        initialization_authority: accounts.protocol_vault,
        protocol_authority: accounts.protocol_vault.to_bytes().into(),
        fee_authority: accounts.protocol_vault.to_bytes().into(),
        tree_creation_authority: accounts.tree_vault.to_bytes().into(),
        tree_creation_is_permissionless: false,
        forester_authority: accounts.forester_vault.to_bytes().into(),
        ring_creation_authority: accounts.ring_vault.to_bytes().into(),
        ring_activation_is_permissionless: false,
        spl_interface_creation_is_permissionless: false,
    }
    .instruction();
    let create_config_sync = smart_account::execute_sync_ix(
        &accounts.protocol_settings,
        0,
        &[authority.pubkey()],
        &[create_config_ix],
    );
    rpc.create_and_send_transaction(
        &[create_config_sync],
        payer_address,
        &[&payer, &authority],
        ComputeBudgetConfig::for_instruction_count(1),
    )?;

    let tree_creation = create_tree_instructions(
        &rpc,
        &payer.pubkey(),
        &accounts.tree_vault,
        nullifier_tree_params(),
        default_tree_fees(nullifier_tree_params().input_queue_zkp_batch_size)
            .expect("default tree fees"),
    )?;
    let create_tree_syncs = smart_account::execute_sync_each(
        &accounts.tree_settings,
        0,
        &[tree_creation_authority.pubkey()],
        &tree_creation.instructions,
    );
    rpc.create_and_send_transaction(
        &create_tree_syncs,
        payer_address,
        &[&payer, &tree_creation_authority],
        ComputeBudgetConfig::for_instruction_count(create_tree_syncs.len()),
    )?;

    let tree = tree_creation.tree;
    let tree_id = zolana_test_utils::nullifier_pda::tree_id(&rpc, &tree)?;

    // SOL only: asset id 1 is a built-in AssetRegistry::default() entry, no
    // SPL registration needed.
    let assets = AssetRegistry::default();

    let creator_solana_keypair = Keypair::new();
    let creator_seed: [u8; 32] = creator_solana_keypair.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    let creator_shielded_keypair =
        ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&creator_seed))?;
    rpc.airdrop(&creator_solana_keypair.pubkey(), 10_000_000_000)?;

    Deposit::new(DepositParams {
        recipient: &creator_shielded_keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: SHIELD_AMOUNT,
        spl_token_account: None,
        spl_token_program: None,
        memo: None,
    })?
    .send(&rpc, &payer, tree, &payer)?;

    let creator_address = creator_shielded_keypair
        .shielded_address()
        .map_err(|e| anyhow!("creator address: {e:?}"))?;

    // The deposit above already confirmed on-chain, and `sync_wallet` waits
    // for indexer freshness by default (and handles proofless-deposit
    // discovery internally), so one sync is enough -- no manual poll loop
    // needed.
    let mut creator_wallet = Wallet::new(creator_address, assets.clone())
        .map_err(|e| anyhow!("creator wallet: {e:?}"))?;
    sync_wallet(&mut creator_wallet, &creator_shielded_keypair, &indexer)
        .map_err(|e| anyhow!("sync creator deposit: {e:?}"))?;

    let client = ZolanaClient::new(
        rpc,
        indexer,
        ProverClient::default(),
        AsyncZolanaIndexer::new(indexer_url),
        AsyncProverClient::default(),
        Address::new_from_array(tree.to_bytes()),
    );

    Ok(TestEnv {
        client,
        tree,
        tree_id,
        creator: TestWallet {
            wallet: creator_wallet,
            keypair: creator_shielded_keypair,
            solana_keypair: creator_solana_keypair,
        },
        _services: Some(services),
    })
}

fn setup_devnet() -> Result<TestEnv> {
    let rpc_url = std::env::var("ESCROW_DEVNET_RPC_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    let indexer_url = std::env::var("ZOLANA_INDEXER_URL")
        .unwrap_or_else(|_| "https://d2xah7tnhdhcom.cloudfront.net".to_string());
    let prover_url = std::env::var("ZOLANA_PROVER_URL")
        .unwrap_or_else(|_| "https://d21ni15goiip6l.cloudfront.net".to_string());
    std::env::set_var("ZOLANA_PROVER_URL", &prover_url);
    let rpc = SolanaRpc::new(rpc_url);
    let indexer = ZolanaIndexer::new(indexer_url.clone());

    let spp_program = Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID);
    rpc.assert_executable(&spp_program)?;
    let escrow_program = Pubkey::new_from_array(*timelock_escrow_program::ID.as_array());
    rpc.assert_executable(&escrow_program)?;

    let tree = zolana_interface::pda::tree(0);
    let tree_id = zolana_test_utils::nullifier_pda::tree_id(&rpc, &tree)?;
    let creator_keypair_path = std::env::var("ESCROW_DEVNET_KEYPAIR")
        .map_err(|_| anyhow!("set ESCROW_DEVNET_KEYPAIR to a funded devnet keypair"))?;
    let creator_solana_keypair = read_keypair_file(&creator_keypair_path)
        .map_err(|error| anyhow!("read devnet keypair {creator_keypair_path}: {error}"))?;
    let creator_seed: [u8; 32] = creator_solana_keypair.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    let creator_shielded_keypair =
        ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&creator_seed))?;

    let creator_address = creator_shielded_keypair
        .shielded_address()
        .map_err(|error| anyhow!("creator address: {error:?}"))?;
    let mut creator_wallet = Wallet::new(creator_address, AssetRegistry::default())
        .map_err(|error| anyhow!("creator wallet: {error:?}"))?;
    sync_wallet(&mut creator_wallet, &creator_shielded_keypair, &indexer)
        .map_err(|error| anyhow!("sync fresh devnet wallet: {error:?}"))?;
    anyhow::ensure!(
        creator_wallet
            .balance(SOL_MINT, None)
            .map_or(0, |balance| balance.amount)
            == 0,
        "ESCROW_DEVNET_KEYPAIR must be a fresh shielded wallet"
    );

    Deposit::new(DepositParams {
        recipient: &creator_shielded_keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: SHIELD_AMOUNT,
        spl_token_account: None,
        spl_token_program: None,
        memo: None,
    })?
    .send(&rpc, &creator_solana_keypair, tree, &creator_solana_keypair)?;

    let mut synced_deposit = false;
    let mut sync_error = "deposit has not been indexed".to_string();
    for _ in 0..60 {
        match sync_wallet(&mut creator_wallet, &creator_shielded_keypair, &indexer) {
            Ok(_) => {
                if creator_wallet
                    .balance(SOL_MINT, None)
                    .is_ok_and(|balance| balance.amount >= SHIELD_AMOUNT)
                {
                    synced_deposit = true;
                    break;
                }
            }
            Err(error) => sync_error = format!("{error:?}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if !synced_deposit {
        return Err(anyhow!("sync creator deposit: {sync_error}"));
    }

    let client = ZolanaClient::new(
        rpc,
        indexer,
        ProverClient::new(prover_url.clone()),
        AsyncZolanaIndexer::new(indexer_url),
        AsyncProverClient::new(prover_url),
        Address::new_from_array(tree.to_bytes()),
    );

    Ok(TestEnv {
        client,
        tree,
        tree_id,
        creator: TestWallet {
            wallet: creator_wallet,
            keypair: creator_shielded_keypair,
            solana_keypair: creator_solana_keypair,
        },
        _services: None,
    })
}

// Submit the forwarded SPP instruction as a transaction-v1 message, which has
// enough account space for the tree and nullifier PDA accounts.
pub fn send(rpc: &SolanaRpc, payer: &dyn Signer, ix: Instruction) -> Result<Signature> {
    Ok(rpc.create_and_send_transaction(
        std::slice::from_ref(&ix),
        payer.pubkey(),
        &[payer],
        ComputeBudgetConfig::new(TRANSACT_COMPUTE_UNIT_LIMIT),
    )?)
}

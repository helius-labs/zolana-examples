mod services;

use std::time::Duration;

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_address_lookup_table_interface::instruction::{
    create_lookup_table, extend_lookup_table,
};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::{versioned::VersionedTransaction, Transaction};
use zolana_client::{
    AsyncProverClient, AsyncZolanaIndexer, ProverClient, Rpc, SolanaRpc, ZolanaClient,
    ZolanaIndexer,
};
use zolana_interface::{
    instruction::{CreateProtocolConfig, CreateTree},
    pda,
    state::tree_account_size,
    SHIELDED_POOL_PROGRAM_ID,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_program_test::system_create_account_ix;
use zolana_test_utils::smart_account::{self, StandardSigners};
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};
use zolana_wallet::{sync_wallet, Deposit, DepositParams};

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
    pub creator: TestWallet,
    pub _services: services::LocalnetServices,
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

pub fn setup() -> Result<TestEnv> {
    let services = services::LocalnetServices::start()?;
    let rpc_url = services.rpc_url.clone();
    let indexer_url = services.indexer_url.clone();
    let prover_url = services.prover_url.clone();
    let mut rpc = SolanaRpc::new(rpc_url);
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
        rpc.create_and_send_transaction(&[ix], payer_address, &[&payer])?;
    }

    rpc.airdrop(&accounts.protocol_vault, 5_000_000_000)?;

    let create_config_ix = CreateProtocolConfig {
        authority: accounts.protocol_vault,
        protocol_authority: accounts.protocol_vault.to_bytes().into(),
        tree_creation_authority: accounts.tree_vault.to_bytes().into(),
        tree_creation_is_permissionless: false,
        forester_authority: accounts.forester_vault.to_bytes().into(),
        ring_creation_authority: accounts.ring_vault.to_bytes().into(),
        ring_creation_is_permissionless: false,
        spl_interface_creation_is_permissionless: false,
    }
    .instruction();
    let create_config_sync = smart_account::execute_sync_ix(
        &accounts.protocol_settings,
        0,
        &[authority.pubkey()],
        &[create_config_ix],
    );
    rpc.create_and_send_transaction(&[create_config_sync], payer_address, &[&payer, &authority])?;

    let tree = Keypair::new();
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(tree_account_size())
        .map_err(|e| anyhow!("{e}"))?;
    let alloc_ix = system_create_account_ix(
        &payer.pubkey(),
        &tree.pubkey(),
        rent,
        tree_account_size() as u64,
        &pda::shielded_pool_program_id(),
    );
    let create_tree_ix = CreateTree {
        authority: accounts.tree_vault,
        tree: tree.pubkey(),
    }
    .instruction();
    let create_tree_sync = smart_account::execute_sync_ix(
        &accounts.tree_settings,
        0,
        &[tree_creation_authority.pubkey()],
        &[create_tree_ix],
    );
    rpc.create_and_send_transaction(
        &[alloc_ix, create_tree_sync],
        payer_address,
        &[&payer, &tree, &tree_creation_authority],
    )?;

    let tree = tree.pubkey();

    // SOL only: asset id 1 is a built-in AssetRegistry::default() entry, no
    // SPL registration needed.
    let assets = AssetRegistry::default();

    let creator_solana_keypair = Keypair::new();
    let creator_seed: [u8; 32] = creator_solana_keypair.to_bytes()[..32]
        .try_into()
        .expect("ed25519 seed is the first 32 bytes");
    let creator_shielded_keypair = ShieldedKeypair::from_ed25519(&creator_seed, ViewingKey::new())?;
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
        ProverClient::new(prover_url.clone()),
        AsyncZolanaIndexer::new(indexer_url),
        AsyncProverClient::new(prover_url),
        Address::new_from_array(tree.to_bytes()),
    );

    Ok(TestEnv {
        _services: services,
        client,
        tree,
        creator: TestWallet {
            wallet: creator_wallet,
            keypair: creator_shielded_keypair,
        },
    })
}

// Submit a single (large) instruction as a v0 transaction behind a throwaway
// address lookup table: create + extend the ALT (waiting a slot for each to
// root), then compile and send. Prepends a 1.4M CU budget; `payer` signs and
// pays. The escrow/withdraw account lists (forwarding the SPP transact's tree
// accounts) don't fit within the 1232-byte tx limit via a legacy transaction.
pub fn send_v0_with_lookup_table(
    rpc: &SolanaRpc,
    payer: &Keypair,
    ix: Instruction,
) -> Result<Signature> {
    let alt_addresses: Vec<Pubkey> = ix
        .accounts
        .iter()
        .filter(|meta| !meta.is_signer)
        .map(|meta| meta.pubkey)
        .chain(std::iter::once(ix.program_id))
        .collect();
    let compute = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);

    let client = rpc.client();
    let recent_slot = client.get_slot().map_err(|e| anyhow!("get_slot: {e}"))?;
    loop {
        let tip = client.get_slot().map_err(|e| anyhow!("get_slot: {e}"))?;
        if tip > recent_slot {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (lut_create_ix, table_address) =
        create_lookup_table(payer.pubkey(), payer.pubkey(), recent_slot);
    let lut_extend_ix = extend_lookup_table(
        table_address,
        payer.pubkey(),
        Some(payer.pubkey()),
        alt_addresses.clone(),
    );
    let blockhash = client
        .get_latest_blockhash()
        .map_err(|e| anyhow!("blockhash: {e}"))?;
    let setup = Transaction::new(
        &[payer],
        Message::new(&[lut_create_ix, lut_extend_ix], Some(&payer.pubkey())),
        blockhash,
    );
    client
        .send_and_confirm_transaction(&setup)
        .map_err(|e| anyhow!("create+extend ALT: {e}"))?;
    let extended_slot = client.get_slot().map_err(|e| anyhow!("get_slot: {e}"))?;
    loop {
        let tip = client.get_slot().map_err(|e| anyhow!("get_slot: {e}"))?;
        if tip > extended_slot {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let alt = AddressLookupTableAccount {
        key: table_address,
        addresses: alt_addresses.clone(),
    };
    let blockhash = client
        .get_latest_blockhash()
        .map_err(|e| anyhow!("blockhash: {e}"))?;
    let message = v0::Message::try_compile(
        &payer.pubkey(),
        &[compute, ix],
        std::slice::from_ref(&alt),
        blockhash,
    )
    .map_err(|e| anyhow!("compile v0: {e}"))?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(message), &[payer])
        .map_err(|e| anyhow!("sign v0: {e}"))?;
    let signature = client
        .send_and_confirm_transaction(&tx)
        .map_err(|e| anyhow!("send v0: {e}"))?;
    Ok(signature)
}

//! `execute_batch` end to end in Mollusk: real SPP proofs for both settlement
//! legs, a real settlement proof, the SPP program, and the mainnet stake pool,
//! token, and stake programs against fabricated pool accounts.
//!
//! Each scenario registers a config, creates and locks an epoch, appends the
//! escrow hashes to chain 0, puts the escrow leaves in the escrow tree,
//! creates and fills the batch, proves, and executes. Failure cases reuse the
//! proven net-deposit and cancelled scenarios.

mod common;

use std::{collections::BTreeMap, sync::Mutex, time::Instant};

use common::{
    non_inclusion_proof, start_prover, wallet, ConfigParams, Harness, SppInterfaces,
    StakePoolFixture, StakePoolParams, TreeFixture, EPOCH, SPP,
};
use mollusk_svm::result::InstructionResult;
use p256::SecretKey;
use private_staking_program::{
    error::StakingError,
    instructions::{
        execute::ExecuteBatchIxData,
        management::{CreateConfigIxData, FillBatchIxData},
    },
    pda,
    settlement::{PoolOperation, SettlementAmounts, SettlementLegs, SppProofData},
    state::{self, Config, EpochHeader, HashChain, IntentUtxoData},
};
use private_staking_sdk::{
    allocation::{allocate, Allocation, IntentAmount},
    encryption::{decrypt_payout, Context, Recipient},
    instructions::{self, PoolAccounts},
    note::{crank_address, recipient_address, Note},
    settlement::{Escrow, SettlementBuilder, SettlementParams},
};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::Instruction;
use spl_stake_pool::state::Fee;
use zolana_client::{ProverClient, STATE_TREE_HEIGHT};
use zolana_hasher::{primitives::right_align, Poseidon};
use zolana_interface::instruction::instruction_data::transact::{TransactProof, TreeContext};
use zolana_keypair::{NullifierKey, PublicKey, ShieldedKeypair, SigningKey};
use zolana_merkle_tree::MerkleTree;
use zolana_test_utils::utxo::ProofKeys;
use zolana_transaction::Mint;
use zolana_tree::TreeAccount;

const TREE_ID: u16 = 0;
const POOL_ASSET_ID: u64 = 1;
const HASH_CHAIN_INDEX: u16 = 0;
const INTENT_FEE: u64 = 50_000;
const MIN_DEPOSIT: u64 = 1_000_000;
const MIN_WITHDRAWAL: u64 = 1_000_000;
/// Settlement happens at unix 1_400 of epoch `EPOCH + 1`, which started at
/// 1_000; a 600 second window gives the deadline 1_600.
const SETTLEMENT_NOW: i64 = 1_400;
const DEADLINE: i64 = 1_600;
const SOL_VAULT_BASELINE: u64 = 5_000_000;
const TOKEN_VAULT_BASELINE: u64 = 7_000_000;
const CU_LIMIT: u64 = 1_400_000;
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

const PAYER: Address = Address::new_from_array([0x71; 32]);
const RENT_PAYER: Address = Address::new_from_array([0x72; 32]);
const ATTACKER: Address = Address::new_from_array([0x73; 32]);

/// Proving takes several GB per settlement proof: one scenario at a time.
static PROVING: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug)]
enum Intent {
    Deposit(u64),
    Withdrawal(u64),
    /// An escrow whose cancellation already spent it.
    Cancelled {
        pool_asset: bool,
        amount: u64,
    },
}

impl Intent {
    fn amount(self) -> IntentAmount {
        match self {
            Self::Deposit(amount) => IntentAmount::Deposit(amount),
            Self::Withdrawal(amount) => IntentAmount::Withdrawal(amount),
            Self::Cancelled { .. } => IntentAmount::Cancelled,
        }
    }

    fn cancelled(self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }
}

struct Scenario {
    h: Harness,
    intents: Vec<Intent>,
    builder: SettlementBuilder,
    allocation: Allocation,
    config: Config,
    tree: Address,
    escrow_leaves: Vec<[u8; 32]>,
    pool: StakePoolFixture,
    interfaces: SppInterfaces,
    crank: ShieldedKeypair,
    recipients: Vec<ShieldedKeypair>,
    batch: Address,
    /// Accounts after `create_batch`, before `fill_batch`.
    unfilled: BTreeMap<Address, Account>,
    ix: Instruction,
}

fn keypair(seed: u8) -> ShieldedKeypair {
    ShieldedKeypair::from_keypair(SigningKey::from_ed25519_bytes(&[seed; 32])).unwrap()
}

fn pool_params() -> StakePoolParams {
    StakePoolParams {
        total_lamports: 1_000 * LAMPORTS_PER_SOL,
        pool_token_supply: 900 * LAMPORTS_PER_SOL,
        deposit_fee: Fee {
            denominator: 100,
            numerator: 1,
        },
        withdrawal_fee: Fee {
            denominator: 1_000,
            numerator: 3,
        },
        reserve_extra_lamports: 50 * LAMPORTS_PER_SOL,
        last_update_epoch: EPOCH + 1,
    }
}

/// Registers, creates and locks the epoch, fills the batch, proves both SPP
/// legs and the settlement, and builds `execute_batch`.
fn scenario(intents: &[Intent]) -> Scenario {
    let mut h = Harness::new();
    let crank = keypair(200);
    let crank_address = crank_address(&crank.shielded_address().unwrap()).unwrap();
    let crank_key = crank_address.signing_pubkey;
    let pool = h.stake_pool(pool_params());
    let tree = zolana_interface::pda::tree(TREE_ID);
    h.tree(tree, &[]);
    h.accounts.extend([
        (PAYER, wallet(1_000 * LAMPORTS_PER_SOL)),
        (RENT_PAYER, wallet(10 * LAMPORTS_PER_SOL)),
        (crank_key, wallet(LAMPORTS_PER_SOL)),
    ]);
    let config_key = h.register(ConfigParams {
        authority: PAYER,
        payer: PAYER,
        stake_pool: pool.pool,
        escrow_tree: tree,
        data: CreateConfigIxData {
            crank: crank_address,
            intent_fee_lamports: INTENT_FEE,
            settlement_window_seconds: 600,
            min_deposit_lamports: MIN_DEPOSIT,
            min_withdrawal_pool_tokens: MIN_WITHDRAWAL,
        },
    });
    let config = h.config_state();
    let epoch = h.create_epoch(EPOCH, 0);

    // Escrow notes owned by the escrow authority with the zero nullifier key.
    let authority = pda::escrow_authority(&config_key).0;
    let zero_pk = NullifierKey::from_secret([0; 31]).pubkey().unwrap();
    let pool_mint = Mint::new(config.pool_mint, POOL_ASSET_ID);
    let mut recipients = Vec::with_capacity(intents.len());
    let mut escrows = Vec::with_capacity(intents.len());
    for ((leaf_index, intent), seed) in (0u64..).zip(intents).zip(20u8..) {
        let recipient = keypair(seed);
        let (asset, amount) = match *intent {
            Intent::Deposit(amount) => (Mint::SOL, amount),
            Intent::Withdrawal(amount) => (pool_mint, amount),
            Intent::Cancelled {
                pool_asset: true,
                amount,
            } => (pool_mint, amount),
            Intent::Cancelled { amount, .. } => (Mint::SOL, amount),
        };
        escrows.push(Escrow {
            note: Note {
                version: 1,
                tree_id: TREE_ID,
                asset: asset.asset,
                asset_id: asset.asset_id,
                amount,
                blinding: right_align(&[seed, 0x5a]),
                signing_pubkey: *PublicKey::from_pda(&authority).as_bytes(),
                nullifier_pubkey: zero_pk,
                intent: Some(IntentUtxoData {
                    epoch,
                    recipient: recipient_address(&recipient.shielded_address().unwrap()),
                }),
            },
            leaf_index,
        });
        recipients.push(recipient);
    }
    let escrow_leaves: Vec<[u8; 32]> = escrows
        .iter()
        .map(|escrow| escrow.note.hash().unwrap())
        .collect();
    h.seed_intent_hashes(&escrow_leaves);
    let tree_fixture = h.tree(tree, &escrow_leaves);

    h.settlement_time(SETTLEMENT_NOW);
    h.run(
        instructions::lock_epoch(crank_key, config_key, epoch),
        Ok(()),
    );
    assert_eq!(h.epoch_state().settlement_deadline, DEADLINE);

    let amounts: Vec<IntentAmount> = intents.iter().map(|intent| intent.amount()).collect();
    let allocation = allocate(&pool.snapshot, &amounts).unwrap();
    let builder = SettlementBuilder::new(SettlementParams {
        config_key,
        config: &config,
        epoch,
        settlement_deadline: DEADLINE,
        hash_chain_index: HASH_CHAIN_INDEX,
        tree_id: TREE_ID,
        payer: PAYER,
        escrows,
        allocation: &allocation,
        pool: pool.snapshot,
        pool_asset_id: POOL_ASSET_ID,
        ephemeral: SecretKey::from_slice(&[4; 32]).unwrap(),
        salt: [5; 16],
        withdrawal_blinding_seed: right_align(&[6, 6, 6]),
        payout_blinding_seed: right_align(&[8, 8, 8]),
    })
    .unwrap();

    // Cancelled escrows as `cancel_intent` leaves them: the nullifier PDA
    // queued by SPP and the intent counted as resolved.
    let mut cancelled = 0;
    for (intent, nullifier) in intents.iter().zip(builder.nullifiers()) {
        if intent.cancelled() {
            h.queued_nullifier(tree, nullifier, TREE_ID);
            cancelled += 1;
        }
    }
    let header = h.epoch_state();
    write_header(
        &mut h,
        EpochHeader {
            resolved_intent_count: cancelled,
            ..header
        },
    );

    h.sol_vault(config_key, SOL_VAULT_BASELINE);
    h.token_vault(config_key, &config, TOKEN_VAULT_BASELINE);
    let interfaces = h.spp_interfaces(
        config.pool_mint,
        allocation.amounts.escrow_sol,
        allocation.amounts.escrow_pool_tokens,
    );
    let batch = h.create_batch(RENT_PAYER, crank_key, builder.create_batch_data());
    let unfilled = h.accounts.clone();
    h.run(
        instructions::fill_batch(
            crank_key,
            epoch,
            batch,
            FillBatchIxData {
                utxos: builder.batch_entries(),
            },
        )
        .unwrap(),
        Ok(()),
    );

    let pool_accounts = PoolAccounts {
        stake_pool_program: spl_stake_pool::id(),
        stake_pool: pool.pool,
        withdraw_authority: pool.withdraw_authority,
        reserve_stake: pool.reserve,
        manager_fee_account: pool.manager_fee,
    };
    let mut ix = prove(&builder, &config, tree, &tree_fixture, &pool_accounts);
    // The batch rent payer differs from the fee payer, so its refund is exact.
    ix.accounts.get_mut(8).unwrap().pubkey = RENT_PAYER;
    Scenario {
        h,
        intents: intents.to_vec(),
        builder,
        allocation,
        config,
        tree,
        escrow_leaves,
        pool,
        interfaces,
        crank,
        recipients,
        batch,
        unfilled,
        ix,
    }
}

fn prove(
    builder: &SettlementBuilder,
    config: &Config,
    tree: Address,
    tree_fixture: &TreeFixture,
    pool_accounts: &PoolAccounts,
) -> Instruction {
    let _guard = PROVING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    start_prover();
    let start = Instant::now();
    let Some(cached) = builder.cached_utxo() else {
        let proof = builder.proof_inputs().unwrap().prove().unwrap();
        println!("settlement proof (all cancelled) in {:?}", start.elapsed());
        return builder.execute_batch(proof, None, pool_accounts).unwrap();
    };
    let zero_key = NullifierKey::from_secret([0; 31]);
    let keys = ProofKeys(&[&zero_key]);
    let spend_proofs = builder
        .spend_targets()
        .into_iter()
        .map(|(_, nullifier, leaf_index)| {
            tree_fixture.spend_proof(tree, usize::try_from(leaf_index).unwrap(), nullifier)
        })
        .collect();
    let padding = builder
        .withdrawal_padding_nullifiers()
        .unwrap()
        .into_iter()
        .map(|nullifier| non_inclusion_proof(tree, nullifier))
        .collect();
    let spend = builder
        .withdrawal_request(spend_proofs, padding)
        .unwrap()
        .prove(&ProverClient::local(), &keys)
        .unwrap();
    let payout = builder
        .payout_request(non_inclusion_proof(tree, cached.nullifier))
        .unwrap()
        .prove(&ProverClient::local(), &keys)
        .unwrap();
    println!("SPP legs proven in {:?}", start.elapsed());
    let bump = zolana_interface::pda::spl_interface_bump(config.pool_mint.as_array());
    let settlement_start = Instant::now();
    let proof = builder
        .settlement_inputs(&spend, &payout, bump)
        .unwrap()
        .prove()
        .unwrap();
    println!("settlement proof in {:?}", settlement_start.elapsed());
    builder
        .execute_batch(proof, Some((spend, payout)), pool_accounts)
        .unwrap()
}

fn write_header(h: &mut Harness, header: EpochHeader) {
    let epoch = h.epoch;
    state::write_epoch(&mut h.accounts.get_mut(&epoch).unwrap().data, &header).unwrap();
}

/// Every account `execute_batch` changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExecutionSnapshot {
    epoch: EpochHeader,
    epoch_lamports: u64,
    chain: HashChain,
    batch_lamports: u64,
    batch_data_len: usize,
    rent_payer_lamports: u64,
    crank_lamports: u64,
    sol_vault_lamports: u64,
    token_vault_tokens: u64,
    sol_interface_lamports: u64,
    spl_interface_tokens: u64,
    cache_lamports: u64,
    /// Owners of the entries' nullifier PDAs (chain order), then the cached
    /// UTXO's when an entry settles.
    nullifier_owners: Vec<Option<Address>>,
    tree_next_index: u64,
    tree_root: [u8; 32],
    pool_total_lamports: u64,
    pool_token_supply: u64,
    manager_fee_tokens: u64,
    reserve_lamports: u64,
}

impl Scenario {
    fn nullifier_pdas(&self) -> Vec<Address> {
        self.builder
            .nullifiers()
            .iter()
            .chain(
                self.builder
                    .cached_utxo()
                    .as_ref()
                    .map(|cached| &cached.nullifier),
            )
            .map(|nullifier| zolana_interface::pda::nullifier_pda(&self.tree, nullifier).0)
            .collect()
    }

    fn snapshot(&self) -> ExecutionSnapshot {
        let h = &self.h;
        let epoch = h.account(&h.epoch);
        let mut tree_bytes = h.account(&self.tree).data.clone();
        let mut tree = TreeAccount::from_bytes(&mut tree_bytes, self.tree.to_bytes()).unwrap();
        let pool = h.pool_state(&self.pool.pool);
        ExecutionSnapshot {
            epoch: state::epoch_from_bytes(&epoch.data).unwrap(),
            epoch_lamports: epoch.lamports,
            chain: state::read_chain(&epoch.data, HASH_CHAIN_INDEX).unwrap(),
            batch_lamports: h.lamports(&self.batch),
            batch_data_len: h
                .accounts
                .get(&self.batch)
                .map_or(0, |account| account.data.len()),
            rent_payer_lamports: h.lamports(&RENT_PAYER),
            crank_lamports: h.lamports(&self.config.crank.signing_pubkey),
            sol_vault_lamports: h.lamports(&pda::sol_vault(&h.config).0),
            token_vault_tokens: h.token_amount(&private_staking_program::pool::token_vault(
                &h.config,
                &self.config,
            )),
            sol_interface_lamports: h.lamports(&self.interfaces.sol_interface),
            spl_interface_tokens: h.token_amount(&self.interfaces.spl_interface),
            cache_lamports: h.lamports(&self.builder.cache()),
            nullifier_owners: self
                .nullifier_pdas()
                .iter()
                .map(|key| h.accounts.get(key).map(|account| account.owner))
                .collect(),
            tree_next_index: tree.utxo_tree().next_index(),
            tree_root: tree.utxo_tree().root(),
            pool_total_lamports: pool.total_lamports,
            pool_token_supply: pool.pool_token_supply,
            manager_fee_tokens: h.token_amount(&self.pool.manager_fee),
            reserve_lamports: h.lamports(&self.pool.reserve),
        }
    }

    /// The post-execution snapshot implied by the allocation and the pool's
    /// SOL deposit and withdrawal rules.
    fn expected(&self, before: &ExecutionSnapshot) -> ExecutionSnapshot {
        let active = self.builder.active_count();
        let fees = u64::try_from(active).unwrap() * INTENT_FEE;
        let amounts = self.allocation.amounts;
        let payouts: Vec<[u8; 32]> = self
            .builder
            .batch_entries()
            .into_iter()
            .filter_map(|(_, output)| output.map(|output| output.utxo_hash))
            .collect();
        let mut leaves = self.escrow_leaves.clone();
        if let Some(cached) = self.builder.cached_utxo() {
            leaves.push(cached.hash);
            leaves.extend(payouts);
        }
        let mut reference = MerkleTree::<Poseidon>::new(STATE_TREE_HEIGHT, 0);
        for leaf in &leaves {
            reference.append(leaf).unwrap();
        }
        let snapshot = self.pool.snapshot;
        let (total, supply, manager_fee, reserve) = match self.allocation.operation {
            PoolOperation::None => (
                before.pool_total_lamports,
                before.pool_token_supply,
                before.manager_fee_tokens,
                before.reserve_lamports,
            ),
            PoolOperation::DepositSol {
                lamports,
                pool_tokens,
            } => {
                let gross = snapshot.gross_deposit_tokens(lamports).unwrap();
                (
                    before.pool_total_lamports + lamports,
                    before.pool_token_supply + gross,
                    before.manager_fee_tokens + gross - pool_tokens,
                    before.reserve_lamports + lamports,
                )
            }
            PoolOperation::WithdrawSol {
                pool_tokens,
                lamports,
            } => {
                let fee =
                    u64::try_from(snapshot.withdrawal_fee.apply(pool_tokens).unwrap()).unwrap();
                (
                    before.pool_total_lamports - lamports,
                    before.pool_token_supply - (pool_tokens - fee),
                    before.manager_fee_tokens + fee,
                    before.reserve_lamports - lamports,
                )
            }
        };
        ExecutionSnapshot {
            epoch: EpochHeader {
                resolved_intent_count: before.epoch.resolved_intent_count
                    + u32::try_from(active).unwrap(),
                fee_balance_lamports: before.epoch.fee_balance_lamports - fees,
                ..before.epoch
            },
            epoch_lamports: before.epoch_lamports - fees,
            chain: HashChain {
                executed: true,
                batch_exists: false,
                ..before.chain
            },
            batch_lamports: 0,
            batch_data_len: 0,
            rent_payer_lamports: before.rent_payer_lamports + before.batch_lamports,
            crank_lamports: before.crank_lamports + fees,
            sol_vault_lamports: before.sol_vault_lamports,
            token_vault_tokens: before.token_vault_tokens,
            sol_interface_lamports: before.sol_interface_lamports - amounts.escrow_sol
                + amounts.payout_sol,
            spl_interface_tokens: before.spl_interface_tokens - amounts.escrow_pool_tokens
                + amounts.payout_pool_tokens,
            cache_lamports: 0,
            nullifier_owners: vec![Some(SPP); before.nullifier_owners.len()],
            tree_next_index: u64::try_from(leaves.len()).unwrap(),
            tree_root: reference.root(),
            pool_total_lamports: total,
            pool_token_supply: supply,
            manager_fee_tokens: manager_fee,
            reserve_lamports: reserve,
        }
    }

    /// Each stored payout decrypts for its recipient and for the crank to
    /// the builder's payout note.
    fn assert_payouts_decrypt(&self) {
        let entries = state::read_entries(&self.h.account(&self.batch).data).unwrap();
        assert_eq!(entries, self.builder.batch_entries());
        let crank_secret =
            SecretKey::from_slice(self.crank.viewing_key.secret_bytes().as_slice()).unwrap();
        let mut output_index = 0u32;
        for ((entry, expected), recipient) in entries
            .iter()
            .zip(self.builder.payout_notes())
            .zip(&self.recipients)
        {
            let (Some(output), Some(expected)) = (&entry.1, expected) else {
                assert!(entry.1.is_none() && expected.is_none());
                continue;
            };
            let context = Context {
                tx_viewing_pk: self.builder.tx_viewing_pk(),
                salt: [5; 16],
                output_index,
                utxo_hash: output.utxo_hash,
            };
            let address = recipient_address(&recipient.shielded_address().unwrap());
            let user_secret =
                SecretKey::from_slice(recipient.viewing_key.secret_bytes().as_slice()).unwrap();
            for (secret, role) in [
                (&user_secret, Recipient::User),
                (&crank_secret, Recipient::Crank),
            ] {
                assert_eq!(
                    decrypt_payout(
                        output.data.as_deref().unwrap(),
                        secret,
                        role,
                        &context,
                        &address,
                        self.config.pool_mint,
                        POOL_ASSET_ID,
                    )
                    .unwrap(),
                    expected
                );
            }
            output_index += 1;
        }
    }

    /// Run `ix` expecting `error` and assert no account changed.
    fn reject(&mut self, ix: Instruction, error: StakingError) {
        let before = self.h.accounts.clone();
        self.h.error(ix, error);
        assert_eq!(self.h.accounts, before);
    }

    /// Execute the proven batch and assert the full post-state.
    fn execute(&mut self, name: &str) -> InstructionResult {
        self.assert_payouts_decrypt();
        let before = self.snapshot();
        let expected = self.expected(&before);
        let result = self.h.run(self.ix.clone(), Ok(()));
        println!(
            "{name}: execute_batch consumed {} CU ({} entries, {} active)",
            result.compute_units_consumed,
            self.intents.len(),
            self.builder.active_count()
        );
        assert_eq!(self.snapshot(), expected);
        assert!(result.compute_units_consumed < CU_LIMIT);
        result
    }
}

/// `ix` with its instruction data changed by `edit`.
fn with_data(ix: &Instruction, edit: impl FnOnce(&mut ExecuteBatchIxData)) -> Instruction {
    let (tag, payload) = ix.data.split_first().unwrap();
    let mut data: ExecuteBatchIxData = wincode::deserialize_exact(payload).unwrap();
    edit(&mut data);
    let mut bytes = vec![*tag];
    bytes.extend(wincode::serialize(&data).unwrap());
    Instruction {
        data: bytes,
        ..ix.clone()
    }
}

fn legs_mut(data: &mut ExecuteBatchIxData) -> &mut SettlementLegs {
    data.legs.as_mut().unwrap()
}

fn net_deposit() -> Vec<Intent> {
    vec![
        Intent::Deposit(2 * LAMPORTS_PER_SOL),
        Intent::Deposit(3 * LAMPORTS_PER_SOL),
        Intent::Deposit(5 * LAMPORTS_PER_SOL),
    ]
}

/// Failures reuse the proven net-deposit batch, then it settles.
#[test]
fn net_deposit_settles_through_deposit_sol() {
    let start = Instant::now();
    let mut s = scenario(&net_deposit());
    assert!(matches!(
        s.allocation.operation,
        PoolOperation::DepositSol { .. }
    ));
    let ix = s.ix.clone();

    // A quote the pool does not give.
    let bad_quote = with_data(&ix, |data| legs_mut(data).amounts.payout_pool_tokens += 1);
    s.reject(bad_quote, StakingError::InvalidPool);

    // A consistent quote for amounts the proof does not cover.
    let snapshot = s.pool.snapshot;
    let unproven = with_data(&ix, |data| {
        let legs = legs_mut(data);
        let escrow_sol = legs.amounts.escrow_sol + LAMPORTS_PER_SOL;
        legs.amounts = SettlementAmounts {
            escrow_sol,
            payout_pool_tokens: snapshot.deposit_tokens(escrow_sol).unwrap(),
            ..legs.amounts
        };
    });
    s.reject(unproven, StakingError::ProofVerificationFailed);

    // The settlement window closes at the deadline.
    s.h.settlement_time(DEADLINE);
    s.reject(ix.clone(), StakingError::SettlementExpired);
    s.h.settlement_time(SETTLEMENT_NOW);

    // An unlocked epoch cannot settle.
    let header = s.h.epoch_state();
    write_header(
        &mut s.h,
        EpochHeader {
            locked: false,
            ..header
        },
    );
    s.reject(ix.clone(), StakingError::EpochUnlocked);
    write_header(&mut s.h, header);

    // Fees go to the batch's crank only.
    let mut wrong_crank = ix.clone();
    wrong_crank.accounts.get_mut(7).unwrap().pubkey = ATTACKER;
    s.reject(wrong_crank, StakingError::InvalidCrank);

    // A batch holding fewer entries than the chain.
    let filled = std::mem::replace(&mut s.h.accounts, s.unfilled.clone());
    let crank_key = s.config.crank.signing_pubkey;
    let partial: Vec<_> = s.builder.batch_entries().into_iter().take(2).collect();
    s.h.run(
        instructions::fill_batch(
            crank_key,
            s.h.epoch,
            s.batch,
            FillBatchIxData { utxos: partial },
        )
        .unwrap(),
        Ok(()),
    );
    s.reject(ix.clone(), StakingError::IncompleteBatch);
    s.h.accounts = filled;

    s.execute("net deposit");
    println!("net deposit test in {:?}", start.elapsed());
}

#[test]
fn net_withdrawal_settles_through_withdraw_sol() {
    let start = Instant::now();
    let mut s = scenario(&[
        Intent::Withdrawal(LAMPORTS_PER_SOL),
        Intent::Withdrawal(2 * LAMPORTS_PER_SOL),
        Intent::Withdrawal(3 * LAMPORTS_PER_SOL / 2),
    ]);
    assert!(matches!(
        s.allocation.operation,
        PoolOperation::WithdrawSol { .. }
    ));
    s.execute("net withdrawal");
    println!("net withdrawal test in {:?}", start.elapsed());
}

#[test]
fn balanced_batch_leaves_the_pool_unchanged() {
    let start = Instant::now();
    // 9 SOL of pool tokens are worth exactly 10 SOL at 1000/900.
    let mut s = scenario(&[
        Intent::Deposit(4 * LAMPORTS_PER_SOL),
        Intent::Withdrawal(9 * LAMPORTS_PER_SOL / 2),
        Intent::Deposit(6 * LAMPORTS_PER_SOL),
        Intent::Withdrawal(9 * LAMPORTS_PER_SOL / 2),
    ]);
    assert_eq!(s.allocation.operation, PoolOperation::None);
    let pool_before = s.h.account(&s.pool.pool).clone();
    s.execute("balanced");
    assert_eq!(s.h.account(&s.pool.pool), &pool_before);
    println!("balanced test in {:?}", start.elapsed());
}

#[test]
fn cancelled_entry_is_skipped() {
    let start = Instant::now();
    let mut s = scenario(&[
        Intent::Deposit(2 * LAMPORTS_PER_SOL),
        Intent::Cancelled {
            pool_asset: false,
            amount: LAMPORTS_PER_SOL,
        },
        Intent::Deposit(4 * LAMPORTS_PER_SOL),
    ]);
    // A cancelled entry needs the nullifier PDA its cancellation created.
    let marker = s.nullifier_pdas().into_iter().nth(1).unwrap();
    let account = s.h.accounts.remove(&marker).unwrap();
    s.reject(s.ix.clone(), StakingError::InvalidCancellation);
    s.h.accounts.insert(marker, account);
    s.execute("one cancelled");
    println!("cancelled entry test in {:?}", start.elapsed());
}

#[test]
fn all_cancelled_batch_closes_without_spp() {
    let start = Instant::now();
    let mut s = scenario(&[
        Intent::Cancelled {
            pool_asset: false,
            amount: 2 * LAMPORTS_PER_SOL,
        },
        Intent::Cancelled {
            pool_asset: true,
            amount: 3 * LAMPORTS_PER_SOL,
        },
    ]);
    assert!(s.builder.cached_utxo().is_none());
    // Legs with no entry to settle.
    let empty = SppProofData {
        proof: TransactProof::zeroed(),
        private_tx_hash: [0; 32],
    };
    let with_legs = with_data(&s.ix, |data| {
        data.legs = Some(SettlementLegs {
            amounts: SettlementAmounts::default(),
            escrow_tree_context: TreeContext {
                utxo_tree_root_index: 0,
                nullifier_tree_root_index: 0,
            },
            payout_nullifier_root_index: 0,
            withdrawal_output_seed: [0; 32],
            spend: empty,
            payout: empty,
        });
    });
    s.reject(with_legs, StakingError::InvalidInstructionData);
    let pool_before = s.h.account(&s.pool.pool).clone();
    s.execute("all cancelled");
    assert_eq!(s.h.account(&s.pool.pool), &pool_before);
    println!("all cancelled test in {:?}", start.elapsed());
}

/// Sixteen active entries with both assets: the compute budget's worst case.
#[test]
fn full_batch_fits_the_compute_budget() {
    let start = Instant::now();
    let intents: Vec<Intent> = (1u64..=16)
        .map(|index| {
            if index % 3 == 0 {
                Intent::Withdrawal(index * LAMPORTS_PER_SOL / 10)
            } else {
                Intent::Deposit(index * LAMPORTS_PER_SOL / 4)
            }
        })
        .collect();
    let mut s = scenario(&intents);
    assert_eq!(s.builder.active_count(), 16);
    s.execute("full batch");
    println!("full batch test in {:?}", start.elapsed());
}

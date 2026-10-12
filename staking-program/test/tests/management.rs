use std::collections::BTreeMap;

use mollusk_svm::Mollusk;
use private_staking_program::{
    error::StakingError,
    instructions::management::{
        CreateBatchIxData, CreateConfigIxData, FillBatchIxData, ResetBatchIxData,
    },
    pda,
    state::{self, ShieldedAddress},
    ID, M, N,
};
use private_staking_sdk::instructions;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{error::InstructionError, Instruction};
use zolana_interface::{
    instruction::instruction_data::transact::{InputUtxo, OwnerTag, TransactOutput},
    state::{
        default_tree_fees, discriminator::TREE_ACCOUNT_DISCRIMINATOR, nullifier_tree_params,
        tree_account_size, STATE_HEIGHT,
    },
    SHIELDED_POOL_PROGRAM_ID,
};
use zolana_tree::TreeAccount;

const PAYER: Address = Address::new_from_array([1; 32]);
const CRANK: Address = Address::new_from_array([2; 32]);
const POOL: Address = Address::new_from_array([3; 32]);
const TREE: Address = Address::new_from_array([4; 32]);
const ATTACKER: Address = Address::new_from_array([5; 32]);
const EPOCH: u64 = 42;

fn wallet(lamports: u64) -> Account {
    Account {
        lamports,
        owner: Address::default(),
        ..Account::default()
    }
}

struct Harness {
    svm: Mollusk,
    accounts: BTreeMap<Address, Account>,
    config: Address,
    epoch: Address,
}

impl Harness {
    fn new() -> Self {
        let program = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/deploy/private_staking_program"
        );
        let mut svm = Mollusk::new(&ID, program);
        svm.sysvars.clock.epoch = EPOCH;
        svm.sysvars.clock.slot = svm.sysvars.epoch_schedule.get_first_slot_in_epoch(EPOCH);
        svm.sysvars.clock.unix_timestamp = 500;
        let config = pda::config(&POOL).0;
        let epoch = pda::epoch(&config, EPOCH, 0).0;
        let mut tree_data = vec![0; tree_account_size()];
        TreeAccount::init(
            &mut tree_data,
            TREE_ACCOUNT_DISCRIMINATOR,
            STATE_HEIGHT as u8,
            TREE.to_bytes(),
            0,
            nullifier_tree_params(),
            default_tree_fees(nullifier_tree_params().input_queue_zkp_batch_size).unwrap(),
        )
        .unwrap();
        let pool = spl_stake_pool::state::StakePool {
            account_type: spl_stake_pool::state::AccountType::StakePool,
            pool_mint: Address::new_from_array([6; 32]),
            token_program_id: Address::new_from_array([7; 32]),
            ..Default::default()
        };
        let accounts = BTreeMap::from([
            (PAYER, wallet(10_000_000_000)),
            (CRANK, wallet(1_000_000)),
            (ATTACKER, wallet(1_000_000)),
            (
                POOL,
                Account {
                    lamports: 10_000_000,
                    data: borsh::to_vec(&pool).unwrap(),
                    owner: spl_stake_pool::id(),
                    ..Account::default()
                },
            ),
            (
                TREE,
                Account {
                    lamports: 1_000_000_000_000,
                    data: tree_data,
                    owner: Address::new_from_array(SHIELDED_POOL_PROGRAM_ID),
                    ..Account::default()
                },
            ),
            (pda::rent_payer(&config).0, wallet(1_000_000_000)),
        ]);
        Self {
            svm,
            accounts,
            config,
            epoch,
        }
    }

    fn run(&mut self, ix: Instruction, expected: Result<(), InstructionError>) {
        let keys: Vec<_> = ix
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let accounts: Vec<_> = keys
            .into_iter()
            .map(|key| {
                if key == Address::default() {
                    mollusk_svm::program::keyed_account_for_system_program()
                } else {
                    (
                        key,
                        self.accounts
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| wallet(0)),
                    )
                }
            })
            .collect();
        let result = self.svm.process_instruction(&ix, &accounts);
        assert_eq!(result.raw_result, expected);
        if result.raw_result.is_ok() {
            self.accounts.extend(result.resulting_accounts);
        }
    }

    fn error(&mut self, ix: Instruction, error: StakingError) {
        self.run(ix, Err(InstructionError::Custom(error as u32)));
    }

    fn register(&mut self) {
        self.run(
            instructions::create_config(
                PAYER,
                PAYER,
                POOL,
                TREE,
                CreateConfigIxData {
                    crank: ShieldedAddress {
                        signing_pubkey: CRANK,
                        nullifier_pubkey: [8; 32],
                        viewing_pubkey: [2; 33],
                    },
                    intent_fee_lamports: 50_000,
                    settlement_window_seconds: 600,
                },
            )
            .unwrap(),
            Ok(()),
        );
    }

    fn create_epoch(&mut self) {
        self.run(
            instructions::create_epoch(self.config, EPOCH, 0).unwrap(),
            Ok(()),
        );
    }

    fn settlement_time(&mut self, timestamp: i64) {
        self.svm.sysvars.clock.epoch = EPOCH + 1;
        self.svm.sysvars.clock.slot = self
            .svm
            .sysvars
            .epoch_schedule
            .get_first_slot_in_epoch(EPOCH + 1);
        self.svm.sysvars.clock.epoch_start_timestamp = 1_000;
        self.svm.sysvars.clock.unix_timestamp = timestamp;
    }

    /// Seed escrow commitments here to exercise the management state machine.
    /// End-to-end tests create them through create_intent and SPP instead.
    fn seed_intents(&mut self, count: usize) {
        let epoch = self.accounts.get_mut(&self.epoch).unwrap();
        for index in 0..count {
            let mut hash = [0; 32];
            hash[24..].copy_from_slice(&(index as u64 + 1).to_be_bytes());
            state::append_intent(&mut epoch.data, hash, 50_000).unwrap();
            epoch.lamports += 50_000;
        }
    }

    fn batch(&self) -> Address {
        pda::batch(&self.epoch, 0).0
    }

    fn create_batch(&mut self) {
        self.run(
            instructions::create_batch(
                PAYER,
                CRANK,
                self.config,
                self.epoch,
                CreateBatchIxData {
                    hash_chain_index: 0,
                    tx_viewing_pk: [2; 33],
                    salt: [1; 16],
                },
            )
            .unwrap(),
            Ok(()),
        );
    }
}

#[test]
fn prefunded_pdas_do_not_block_config_or_epoch_creation() {
    let mut h = Harness::new();
    h.accounts.insert(h.config, wallet(1));
    h.accounts.insert(h.epoch, wallet(1));
    h.register();
    let config = state::config_from_bytes(&h.accounts[&h.config].data).unwrap();
    assert_eq!(config.escrow_tree, TREE);
    let rent_before = h.accounts[&pda::rent_payer(&h.config).0].lamports;
    h.create_epoch();
    assert_eq!(h.accounts[&h.epoch].data.len(), state::EPOCH_SIZE);
    let header = state::epoch_from_bytes(&h.accounts[&h.epoch].data).unwrap();
    assert_eq!(
        header.submission_end_slot,
        h.svm
            .sysvars
            .epoch_schedule
            .get_first_slot_in_epoch(EPOCH + 1)
    );
    assert_eq!(header.settlement_deadline, 0);
    assert_eq!(
        rent_before - h.accounts[&pda::rent_payer(&h.config).0].lamports,
        h.accounts[&h.epoch].lamports - 1
    );
}

#[test]
fn only_the_crank_can_lock_and_the_window_is_anchored_to_epoch_start() {
    let mut h = Harness::new();
    h.register();
    h.create_epoch();
    h.error(
        instructions::lock_epoch(CRANK, h.config, h.epoch),
        StakingError::SubmissionsOpen,
    );
    h.settlement_time(1_400);
    h.error(
        instructions::lock_epoch(ATTACKER, h.config, h.epoch),
        StakingError::InvalidCrank,
    );
    h.run(instructions::lock_epoch(CRANK, h.config, h.epoch), Ok(()));
    assert_eq!(
        state::epoch_from_bytes(&h.accounts[&h.epoch].data)
            .unwrap()
            .settlement_deadline,
        1_600
    );
    h.error(
        instructions::unlock_epoch(h.epoch),
        StakingError::SettlementNotExpired,
    );
    h.settlement_time(1_600);
    h.run(instructions::unlock_epoch(h.epoch), Ok(()));
    assert!(
        !state::epoch_from_bytes(&h.accounts[&h.epoch].data)
            .unwrap()
            .locked
    );
    h.error(
        instructions::lock_epoch(CRANK, h.config, h.epoch),
        StakingError::SettlementExpired,
    );
}

#[test]
fn delayed_crank_cannot_start_a_new_window() {
    let mut h = Harness::new();
    h.register();
    h.create_epoch();
    h.settlement_time(1_601);
    h.error(
        instructions::lock_epoch(CRANK, h.config, h.epoch),
        StakingError::SettlementExpired,
    );
    assert!(
        !state::epoch_from_bytes(&h.accounts[&h.epoch].data)
            .unwrap()
            .locked
    );
}

#[test]
fn next_page_requires_the_previous_page_to_be_full() {
    let mut h = Harness::new();
    h.register();
    h.create_epoch();
    h.error(
        instructions::create_epoch(h.config, EPOCH, 1).unwrap(),
        StakingError::InvalidEpoch,
    );
    h.seed_intents(M * N);
    h.run(
        instructions::create_epoch(h.config, EPOCH, 1).unwrap(),
        Ok(()),
    );
    let next = pda::epoch(&h.config, EPOCH, 1).0;
    let header = state::epoch_from_bytes(&h.accounts[&next].data).unwrap();
    assert_eq!(header.page_index, 1);
    assert_eq!(header.intent_count, 0);
}

fn entry(index: u8, active: bool) -> state::BatchEntry {
    (
        InputUtxo {
            nullifier_hash: [index; 32],
            tree_index: 0,
        },
        active.then_some(TransactOutput {
            utxo_hash: [index; 32],
            owner_tag: OwnerTag::Inline([index; 32]),
            data: Some(vec![index; state::MAX_CIPHERTEXT_BYTES]),
        }),
    )
}

#[test]
fn batch_fill_is_bounded_and_reset_requires_fresh_encryption_salt() {
    let mut h = Harness::new();
    h.register();
    h.create_epoch();
    h.seed_intents(2);
    h.settlement_time(1_001);
    h.run(instructions::lock_epoch(CRANK, h.config, h.epoch), Ok(()));
    h.accounts.insert(h.batch(), wallet(1));
    h.create_batch();
    let batch = h.batch();
    let rent = h.accounts[&batch].lamports;
    let fill = FillBatchIxData {
        utxos: vec![entry(1, true), entry(2, false)],
    };
    h.error(
        instructions::fill_batch(ATTACKER, h.epoch, batch, fill.clone()).unwrap(),
        StakingError::InvalidCrank,
    );
    h.run(
        instructions::fill_batch(CRANK, h.epoch, batch, fill).unwrap(),
        Ok(()),
    );
    assert_eq!(h.accounts[&batch].data.len(), state::BATCH_SIZE);
    assert_eq!(
        state::read_entry(&h.accounts[&batch].data, 1).unwrap(),
        entry(2, false)
    );
    h.error(
        instructions::fill_batch(
            CRANK,
            h.epoch,
            batch,
            FillBatchIxData {
                utxos: vec![entry(3, true)],
            },
        )
        .unwrap(),
        StakingError::BatchFull,
    );
    h.error(
        instructions::reset_batch(
            CRANK,
            h.epoch,
            batch,
            ResetBatchIxData {
                tx_viewing_pk: [3; 33],
                salt: [1; 16],
            },
        )
        .unwrap(),
        StakingError::InvalidReset,
    );
    h.run(
        instructions::reset_batch(
            CRANK,
            h.epoch,
            batch,
            ResetBatchIxData {
                tx_viewing_pk: [3; 33],
                salt: [2; 16],
            },
        )
        .unwrap(),
        Ok(()),
    );
    assert_eq!(
        state::batch_from_bytes(&h.accounts[&batch].data)
            .unwrap()
            .filled_count,
        0
    );
    assert_eq!(h.accounts[&batch].lamports, rent);
    h.run(
        instructions::fill_batch(
            CRANK,
            h.epoch,
            batch,
            FillBatchIxData {
                utxos: vec![entry(1, false)],
            },
        )
        .unwrap(),
        Ok(()),
    );
    h.settlement_time(1_600);
    h.error(
        instructions::fill_batch(
            CRANK,
            h.epoch,
            batch,
            FillBatchIxData {
                utxos: vec![entry(2, false)],
            },
        )
        .unwrap(),
        StakingError::SettlementExpired,
    );
}

#[test]
fn abandoned_batches_can_be_cleaned_up_before_the_last_intent_is_resolved() {
    let mut h = Harness::new();
    h.register();
    h.create_epoch();
    h.seed_intents(2);
    h.settlement_time(1_001);
    h.error(
        instructions::close_epoch(h.config, h.epoch, CRANK, &[]),
        StakingError::UnresolvedIntents,
    );
    h.run(instructions::lock_epoch(CRANK, h.config, h.epoch), Ok(()));
    h.create_batch();
    let batch = h.batch();
    let batch_rent = h.accounts[&batch].lamports;
    let payer_before = h.accounts[&PAYER].lamports;
    let cleanup = instructions::close_epoch(h.config, h.epoch, CRANK, &[(batch, PAYER)]);
    h.error(cleanup.clone(), StakingError::SettlementNotExpired);
    h.settlement_time(1_600);
    h.run(cleanup, Ok(()));
    assert_eq!(h.accounts[&PAYER].lamports, payer_before + batch_rent);
    assert_eq!(h.accounts[&batch].lamports, 0);
    assert_eq!(h.accounts[&h.epoch].data.len(), state::EPOCH_SIZE);
    assert!(
        !state::read_chain(&h.accounts[&h.epoch].data, 0)
            .unwrap()
            .batch_exists
    );
    let mut header = state::epoch_from_bytes(&h.accounts[&h.epoch].data).unwrap();
    header.resolve(2).unwrap();
    state::write_epoch(&mut h.accounts.get_mut(&h.epoch).unwrap().data, &header).unwrap();
    let epoch_balance = h.accounts[&h.epoch].lamports;
    let rent_before = h.accounts[&pda::rent_payer(&h.config).0].lamports;
    let crank_before = h.accounts[&CRANK].lamports;
    h.run(
        instructions::close_epoch(h.config, h.epoch, CRANK, &[]),
        Ok(()),
    );
    assert_eq!(h.accounts[&CRANK].lamports - crank_before, 100_000);
    assert_eq!(
        h.accounts[&pda::rent_payer(&h.config).0].lamports - rent_before,
        epoch_balance - 100_000
    );
    assert_eq!(h.accounts[&h.epoch].lamports, 0);
}

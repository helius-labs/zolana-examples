mod common;

use common::{wallet, ConfigParams, Harness, EPOCH};
use private_staking_program::{
    error::StakingError,
    instructions::management::{
        CreateBatchIxData, CreateConfigIxData, FillBatchIxData, ResetBatchIxData,
    },
    pda,
    state::{self, ShieldedAddress},
    M, N,
};
use private_staking_sdk::instructions;
use solana_account::Account;
use solana_address::Address;
use zolana_interface::instruction::instruction_data::transact::{
    InputUtxo, OwnerTag, TransactOutput,
};

const PAYER: Address = Address::new_from_array([1; 32]);
const CRANK: Address = Address::new_from_array([2; 32]);
const POOL: Address = Address::new_from_array([3; 32]);
const TREE: Address = Address::new_from_array([4; 32]);
const ATTACKER: Address = Address::new_from_array([5; 32]);

/// Wallets, a minimal stake pool account, and an empty escrow tree; the
/// config and epoch keys are known before `register` / `create_epoch`.
fn setup() -> Harness {
    let mut h = Harness::new();
    let pool = spl_stake_pool::state::StakePool {
        account_type: spl_stake_pool::state::AccountType::StakePool,
        pool_mint: Address::new_from_array([6; 32]),
        token_program_id: Address::new_from_array([7; 32]),
        ..Default::default()
    };
    h.accounts.extend([
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
    ]);
    h.tree(TREE, &[]);
    h.config = pda::config(&POOL).0;
    h.epoch = pda::epoch(&h.config, EPOCH, 0).0;
    h
}

fn register(h: &mut Harness) {
    h.register(ConfigParams {
        authority: PAYER,
        payer: PAYER,
        stake_pool: POOL,
        escrow_tree: TREE,
        data: CreateConfigIxData {
            crank: ShieldedAddress {
                signing_pubkey: CRANK,
                nullifier_pubkey: [8; 32],
                viewing_pubkey: [2; 33],
            },
            intent_fee_lamports: 50_000,
            settlement_window_seconds: 600,
            min_deposit_lamports: 1_000,
            min_withdrawal_pool_tokens: 1_000,
        },
    });
}

fn create_batch(h: &mut Harness) {
    h.create_batch(
        PAYER,
        CRANK,
        CreateBatchIxData {
            hash_chain_index: 0,
            tx_viewing_pk: [2; 33],
            salt: [1; 16],
        },
    );
}

fn lamports(h: &Harness, key: &Address) -> u64 {
    h.account(key).lamports
}

fn data<'a>(h: &'a Harness, key: &Address) -> &'a [u8] {
    &h.account(key).data
}

#[test]
fn prefunded_pdas_do_not_block_config_or_epoch_creation() {
    let mut h = setup();
    h.accounts.insert(h.config, wallet(1));
    h.accounts.insert(h.epoch, wallet(1));
    register(&mut h);
    let config = state::config_from_bytes(data(&h, &h.config)).unwrap();
    assert_eq!(config.escrow_tree, TREE);
    let rent_before = lamports(&h, &pda::rent_payer(&h.config).0);
    h.create_epoch(EPOCH, 0);
    assert_eq!(data(&h, &h.epoch).len(), state::EPOCH_SIZE);
    let header = state::epoch_from_bytes(data(&h, &h.epoch)).unwrap();
    assert_eq!(
        header.submission_end_slot,
        h.svm
            .sysvars
            .epoch_schedule
            .get_first_slot_in_epoch(EPOCH + 1)
    );
    assert_eq!(header.settlement_deadline, 0);
    assert_eq!(
        rent_before - lamports(&h, &pda::rent_payer(&h.config).0),
        lamports(&h, &h.epoch) - 1
    );
}

#[test]
fn only_the_crank_can_lock_and_the_window_is_anchored_to_epoch_start() {
    let mut h = setup();
    register(&mut h);
    h.create_epoch(EPOCH, 0);
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
        state::epoch_from_bytes(data(&h, &h.epoch))
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
    assert!(!state::epoch_from_bytes(data(&h, &h.epoch)).unwrap().locked);
    h.error(
        instructions::lock_epoch(CRANK, h.config, h.epoch),
        StakingError::SettlementExpired,
    );
}

#[test]
fn delayed_crank_cannot_start_a_new_window() {
    let mut h = setup();
    register(&mut h);
    h.create_epoch(EPOCH, 0);
    h.settlement_time(1_601);
    h.error(
        instructions::lock_epoch(CRANK, h.config, h.epoch),
        StakingError::SettlementExpired,
    );
    assert!(!state::epoch_from_bytes(data(&h, &h.epoch)).unwrap().locked);
}

#[test]
fn next_page_requires_the_previous_page_to_be_full() {
    let mut h = setup();
    register(&mut h);
    h.create_epoch(EPOCH, 0);
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
    let header = state::epoch_from_bytes(data(&h, &next)).unwrap();
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
            data: Some(
                core::iter::once(1)
                    .chain(core::iter::repeat_n(
                        index,
                        state::PAYOUT_CIPHERTEXT_BYTES - 1,
                    ))
                    .collect(),
            ),
        }),
    )
}

#[test]
fn batch_fill_is_bounded_and_reset_requires_fresh_encryption_salt() {
    let mut h = setup();
    register(&mut h);
    h.create_epoch(EPOCH, 0);
    h.seed_intents(2);
    h.settlement_time(1_001);
    h.run(instructions::lock_epoch(CRANK, h.config, h.epoch), Ok(()));
    h.accounts.insert(h.batch(0), wallet(1));
    create_batch(&mut h);
    let batch = h.batch(0);
    let rent = lamports(&h, &batch);
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
    assert_eq!(data(&h, &batch).len(), state::BATCH_SIZE);
    assert_eq!(
        state::read_entry(data(&h, &batch), 1).unwrap(),
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
        state::batch_from_bytes(data(&h, &batch))
            .unwrap()
            .filled_count,
        0
    );
    assert_eq!(lamports(&h, &batch), rent);
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
    let mut h = setup();
    register(&mut h);
    h.create_epoch(EPOCH, 0);
    h.seed_intents(2);
    h.settlement_time(1_001);
    h.error(
        instructions::close_epoch(h.config, h.epoch, CRANK, &[]),
        StakingError::UnresolvedIntents,
    );
    h.run(instructions::lock_epoch(CRANK, h.config, h.epoch), Ok(()));
    create_batch(&mut h);
    let batch = h.batch(0);
    let batch_rent = lamports(&h, &batch);
    let payer_before = lamports(&h, &PAYER);
    let cleanup = instructions::close_epoch(h.config, h.epoch, CRANK, &[(batch, PAYER)]);
    h.error(cleanup.clone(), StakingError::SettlementNotExpired);
    h.settlement_time(1_600);
    h.run(cleanup, Ok(()));
    assert_eq!(lamports(&h, &PAYER), payer_before + batch_rent);
    assert_eq!(lamports(&h, &batch), 0);
    assert_eq!(data(&h, &h.epoch).len(), state::EPOCH_SIZE);
    assert!(
        !state::read_chain(data(&h, &h.epoch), 0)
            .unwrap()
            .batch_exists
    );
    let mut header = state::epoch_from_bytes(data(&h, &h.epoch)).unwrap();
    header.resolve(2).unwrap();
    state::write_epoch(&mut h.accounts.get_mut(&h.epoch).unwrap().data, &header).unwrap();
    let epoch_balance = lamports(&h, &h.epoch);
    let rent_before = lamports(&h, &pda::rent_payer(&h.config).0);
    let crank_before = lamports(&h, &CRANK);
    h.run(
        instructions::close_epoch(h.config, h.epoch, CRANK, &[]),
        Ok(()),
    );
    assert_eq!(lamports(&h, &CRANK) - crank_before, 100_000);
    assert_eq!(
        lamports(&h, &pda::rent_payer(&h.config).0) - rent_before,
        epoch_balance - 100_000
    );
    assert_eq!(lamports(&h, &h.epoch), 0);
}

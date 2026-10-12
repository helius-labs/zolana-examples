use borsh::BorshDeserialize;
use pinocchio::{
    cpi::{Seed, Signer},
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use solana_sysvar::Sysvar as SolanaSysvar;
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;

use super::shared::{check_crank, check_key, check_system, close_to, no_more, parse};
use crate::{
    error::StakingError,
    pda,
    state::{self, BatchEntry, BatchHeader, Config, EpochHeader, ShieldedAddress},
    M, N,
};

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CreateConfigIxData {
    pub crank: ShieldedAddress,
    pub intent_fee_lamports: u64,
    pub settlement_window_seconds: u32,
    pub min_deposit_lamports: u64,
    pub min_withdrawal_pool_tokens: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CreateEpochIxData {
    pub page_index: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct CreateBatchIxData {
    pub hash_chain_index: u16,
    pub tx_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct FillBatchIxData {
    pub utxos: Vec<BatchEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ResetBatchIxData {
    pub tx_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
}

fn ensure_empty(account: &AccountView) -> ProgramResult {
    if !account.owned_by(&Address::default()) || account.data_len() != 0 {
        return Err(StakingError::InvalidAccount.into());
    }
    Ok(())
}

pub fn process_create_config(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: CreateConfigIxData = parse(data)?;
    let mut iter = AccountIterator::new(accounts);
    let authority = iter.next_signer("authority")?;
    let payer = iter.next_signer_mut("payer")?;
    let pool_account = iter.next_account("stake_pool")?;
    let tree = iter.next_account("escrow_tree")?;
    let config_account = iter.next_mut("config")?;
    check_system(iter.next_account("system_program")?)?;
    no_more(&iter)?;
    if ix.settlement_window_seconds == 0
        || ix.settlement_window_seconds > 600
        || !matches!(ix.crank.viewing_pubkey.first(), Some(2 | 3))
    {
        return Err(StakingError::InvalidConfig.into());
    }
    if !pool_account.owned_by(&spl_stake_pool::id()) {
        return Err(StakingError::InvalidPool.into());
    }
    let pool =
        spl_stake_pool::state::StakePool::deserialize(&mut pool_account.try_borrow()?.as_ref())
            .map_err(|_| StakingError::InvalidPool)?;
    if !pool.is_valid() {
        return Err(StakingError::InvalidPool.into());
    }
    zolana_program::compression::load_tree_id(tree).map_err(|_| StakingError::InvalidTree)?;
    let (expected, bump) = pda::config(pool_account.address());
    check_key(config_account, &expected)?;
    ensure_empty(config_account)?;
    let vault = pda::sol_vault(&expected).0;
    if pool.sol_deposit_authority.is_some_and(|key| key != vault)
        || pool.sol_withdraw_authority.is_some_and(|key| key != vault)
    {
        return Err(StakingError::InvalidPool.into());
    }
    let bump_bytes = [bump];
    let seeds = [
        Seed::from(pda::CONFIG_SEED),
        Seed::from(pool_account.address().as_ref()),
        Seed::from(&bump_bytes),
    ];
    pinocchio_system::create_account_with_minimum_balance_signed(
        config_account,
        state::CONFIG_SIZE,
        &crate::ID,
        payer,
        None,
        &[Signer::from(&seeds)],
    )?;
    let config = Config {
        discriminator: state::CONFIG_DISCRIMINATOR,
        authority: *authority.address(),
        stake_pool_program_id: spl_stake_pool::id(),
        stake_pool: *pool_account.address(),
        pool_mint: pool.pool_mint,
        token_program_id: pool.token_program_id,
        escrow_tree: *tree.address(),
        crank: ix.crank,
        intent_fee_lamports: ix.intent_fee_lamports,
        settlement_window_seconds: ix.settlement_window_seconds,
        min_deposit_lamports: ix.min_deposit_lamports,
        min_withdrawal_pool_tokens: ix.min_withdrawal_pool_tokens,
    };
    state::write_config(&mut config_account.try_borrow_mut()?, &config)
}

pub fn process_create_epoch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: CreateEpochIxData = parse(data)?;
    let mut iter = AccountIterator::new(accounts);
    let config_account = iter.next_account("config")?;
    state::load_config(config_account)?;
    let epoch_account = iter.next_mut("epoch")?;
    let rent_payer = iter.next_mut("rent_payer")?;
    check_system(iter.next_account("system_program")?)?;
    let clock = Clock::get()?;
    if ix.page_index != 0 {
        let previous = state::load_epoch(iter.next_account("previous_epoch_page")?)?;
        if previous.config != *config_account.address()
            || previous.epoch_id != clock.epoch
            || previous.page_index.checked_add(1) != Some(ix.page_index)
            || previous.intent_count != u32::try_from(M * N).map_err(|_| StakingError::Overflow)?
        {
            return Err(StakingError::InvalidEpoch.into());
        }
    }
    no_more(&iter)?;
    let next_epoch = clock.epoch.checked_add(1).ok_or(StakingError::Overflow)?;
    let schedule = <solana_epoch_schedule::EpochSchedule as SolanaSysvar>::get()?;
    let end_slot = schedule.get_first_slot_in_epoch(next_epoch);
    if clock.slot >= end_slot {
        return Err(StakingError::SubmissionsClosed.into());
    }
    let (expected, bump) = pda::epoch(config_account.address(), clock.epoch, ix.page_index);
    check_key(epoch_account, &expected)?;
    ensure_empty(epoch_account)?;
    let (rent_key, rent_bump) = pda::rent_payer(config_account.address());
    check_key(rent_payer, &rent_key)?;
    if !rent_payer.owned_by(&Address::default()) || rent_payer.data_len() != 0 {
        return Err(StakingError::InvalidAccount.into());
    }
    let epoch_id = clock.epoch.to_le_bytes();
    let page = ix.page_index.to_le_bytes();
    let bump_bytes = [bump];
    let rent_bump_bytes = [rent_bump];
    let seeds = [
        Seed::from(pda::EPOCH_SEED),
        Seed::from(config_account.address().as_ref()),
        Seed::from(&epoch_id),
        Seed::from(&page),
        Seed::from(&bump_bytes),
    ];
    let rent_seeds = [
        Seed::from(pda::RENT_PAYER_SEED),
        Seed::from(config_account.address().as_ref()),
        Seed::from(&rent_bump_bytes),
    ];
    pinocchio_system::create_account_with_minimum_balance_signed(
        epoch_account,
        state::EPOCH_SIZE,
        &crate::ID,
        rent_payer,
        None,
        &[Signer::from(&seeds), Signer::from(&rent_seeds)],
    )?;
    let header = EpochHeader {
        discriminator: state::EPOCH_DISCRIMINATOR,
        config: *config_account.address(),
        epoch_id: clock.epoch,
        page_index: ix.page_index,
        submission_end_slot: end_slot,
        settlement_deadline: 0,
        locked: false,
        intent_count: 0,
        resolved_intent_count: 0,
        fee_balance_lamports: 0,
    };
    state::write_epoch(&mut epoch_account.try_borrow_mut()?, &header)
}

pub fn process_lock_epoch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() {
        return Err(StakingError::InvalidInstructionData.into());
    }
    let mut iter = AccountIterator::new(accounts);
    let crank = iter.next_signer("crank")?;
    let config_account = iter.next_account("config")?;
    let config = state::load_config(config_account)?;
    let epoch_account = iter.next_mut("epoch")?;
    no_more(&iter)?;
    check_crank(crank, &config.crank)?;
    let mut header = state::load_epoch(epoch_account)?;
    if header.config != *config_account.address() {
        return Err(StakingError::InvalidConfig.into());
    }
    let clock = Clock::get()?;
    if clock.slot < header.submission_end_slot {
        return Err(StakingError::SubmissionsOpen.into());
    }
    if clock.epoch
        != header
            .epoch_id
            .checked_add(1)
            .ok_or(StakingError::Overflow)?
    {
        return Err(StakingError::SettlementExpired.into());
    }
    let deadline = clock
        .epoch_start_timestamp
        .checked_add(i64::from(config.settlement_window_seconds))
        .ok_or(StakingError::Overflow)?;
    if clock.unix_timestamp >= deadline {
        return Err(StakingError::SettlementExpired.into());
    }
    if header.locked {
        return Err(StakingError::EpochLocked.into());
    }
    header.locked = true;
    header.settlement_deadline = deadline;
    state::write_epoch(&mut epoch_account.try_borrow_mut()?, &header)
}

pub fn process_unlock_epoch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() {
        return Err(StakingError::InvalidInstructionData.into());
    }
    let mut iter = AccountIterator::new(accounts);
    let epoch_account = iter.next_mut("epoch")?;
    no_more(&iter)?;
    let mut header = state::load_epoch(epoch_account)?;
    if header.locked {
        if Clock::get()?.unix_timestamp < header.settlement_deadline {
            return Err(StakingError::SettlementNotExpired.into());
        }
        header.locked = false;
        state::write_epoch(&mut epoch_account.try_borrow_mut()?, &header)?;
    }
    Ok(())
}

pub fn process_create_batch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: CreateBatchIxData = parse(data)?;
    let mut iter = AccountIterator::new(accounts);
    let payer = iter.next_signer_mut("payer")?;
    let crank = iter.next_signer("crank")?;
    let config_account = iter.next_account("config")?;
    let config = state::load_config(config_account)?;
    let epoch_account = iter.next_mut("epoch")?;
    let batch_account = iter.next_mut("batch")?;
    check_system(iter.next_account("system_program")?)?;
    no_more(&iter)?;
    check_crank(crank, &config.crank)?;
    let epoch = state::load_epoch(epoch_account)?;
    if epoch.config != *config_account.address() {
        return Err(StakingError::InvalidConfig.into());
    }
    epoch.check_settlement(Clock::get()?.unix_timestamp)?;
    let mut chain = state::read_chain(&epoch_account.try_borrow()?, ix.hash_chain_index)?;
    if chain.intent_count == 0 {
        return Err(StakingError::InvalidChain.into());
    }
    if chain.executed {
        return Err(StakingError::ChainExecuted.into());
    }
    if chain.batch_exists {
        return Err(StakingError::BatchExists.into());
    }
    if !matches!(ix.tx_viewing_pk.first(), Some(2 | 3)) {
        return Err(StakingError::InvalidCiphertext.into());
    }
    let (expected, bump) = pda::batch(epoch_account.address(), ix.hash_chain_index);
    check_key(batch_account, &expected)?;
    ensure_empty(batch_account)?;
    let index = ix.hash_chain_index.to_le_bytes();
    let bump_bytes = [bump];
    let seeds = [
        Seed::from(pda::BATCH_SEED),
        Seed::from(epoch_account.address().as_ref()),
        Seed::from(&index),
        Seed::from(&bump_bytes),
    ];
    pinocchio_system::create_account_with_minimum_balance_signed(
        batch_account,
        state::BATCH_SIZE,
        &crate::ID,
        payer,
        None,
        &[Signer::from(&seeds)],
    )?;
    let header = BatchHeader {
        discriminator: state::BATCH_DISCRIMINATOR,
        epoch: *epoch_account.address(),
        hash_chain_index: ix.hash_chain_index,
        utxo_hash_chain: chain.utxo_hash_chain,
        intent_count: chain.intent_count,
        crank: config.crank,
        rent_payer: *payer.address(),
        tx_viewing_pk: ix.tx_viewing_pk,
        salt: ix.salt,
        filled_count: 0,
    };
    state::write_batch(&mut batch_account.try_borrow_mut()?, &header)?;
    chain.batch_exists = true;
    state::write_chain(
        &mut epoch_account.try_borrow_mut()?,
        ix.hash_chain_index,
        &chain,
    )
}

pub(crate) fn check_batch_epoch(epoch_account: &AccountView, batch: &BatchHeader) -> ProgramResult {
    check_key(epoch_account, &batch.epoch)?;
    let header = state::load_epoch(epoch_account)?;
    check_batch_chain(epoch_account, &header, batch)
}

/// `check_batch_epoch` for a caller that already loaded the epoch header and
/// checked `epoch_account` against `batch.epoch`: the settlement window, then
/// the batch's chain.
pub(crate) fn check_batch_chain(
    epoch_account: &AccountView,
    header: &EpochHeader,
    batch: &BatchHeader,
) -> ProgramResult {
    header.check_settlement(Clock::get()?.unix_timestamp)?;
    let chain = state::read_chain(&epoch_account.try_borrow()?, batch.hash_chain_index)?;
    if chain.executed {
        return Err(StakingError::ChainExecuted.into());
    }
    if !chain.batch_exists
        || chain.intent_count != batch.intent_count
        || chain.utxo_hash_chain != batch.utxo_hash_chain
    {
        return Err(StakingError::InvalidChain.into());
    }
    Ok(())
}

pub fn process_fill_batch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: FillBatchIxData = parse(data)?;
    let mut iter = AccountIterator::new(accounts);
    let crank = iter.next_signer("crank")?;
    let epoch_account = iter.next_account("epoch")?;
    let batch_account = iter.next_mut("batch")?;
    no_more(&iter)?;
    let batch = state::load_batch(batch_account)?;
    check_crank(crank, &batch.crank)?;
    check_batch_epoch(epoch_account, &batch)?;
    state::append_entries(&mut batch_account.try_borrow_mut()?, &ix.utxos)
}

pub fn process_reset_batch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: ResetBatchIxData = parse(data)?;
    let mut iter = AccountIterator::new(accounts);
    let crank = iter.next_signer("crank")?;
    let epoch_account = iter.next_account("epoch")?;
    let batch_account = iter.next_mut("batch")?;
    no_more(&iter)?;
    let mut batch = state::load_batch(batch_account)?;
    check_crank(crank, &batch.crank)?;
    check_batch_epoch(epoch_account, &batch)?;
    if !matches!(ix.tx_viewing_pk.first(), Some(2 | 3)) || ix.salt == batch.salt {
        return Err(StakingError::InvalidReset.into());
    }
    batch.tx_viewing_pk = ix.tx_viewing_pk;
    batch.salt = ix.salt;
    batch.filled_count = 0;
    let mut data = batch_account.try_borrow_mut()?;
    data.get_mut(state::BATCH_HEADER_SIZE..)
        .ok_or(StakingError::InvalidAccount)?
        .fill(0);
    state::write_batch(&mut data, &batch)
}

pub fn process_close_epoch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() {
        return Err(StakingError::InvalidInstructionData.into());
    }
    let mut iter = AccountIterator::new(accounts);
    let config_account = iter.next_account("config")?;
    let config = state::load_config(config_account)?;
    let epoch_account = iter.next_mut("epoch")?;
    let rent_payer = iter.next_mut("rent_payer")?;
    let crank = iter.next_mut("crank")?;
    let header = state::load_epoch(epoch_account)?;
    if header.config != *config_account.address() {
        return Err(StakingError::InvalidConfig.into());
    }
    check_key(rent_payer, &pda::rent_payer(config_account.address()).0)?;
    check_key(crank, &config.crank.signing_pubkey)?;
    if crank.address() == epoch_account.address() {
        return Err(StakingError::InvalidCrank.into());
    }
    let clock = Clock::get()?;
    if clock.slot < header.submission_end_slot {
        return Err(StakingError::SubmissionsOpen.into());
    }
    let has_cleanup = !iter.iterator_is_empty();
    if has_cleanup {
        if clock.epoch
            == header
                .epoch_id
                .checked_add(1)
                .ok_or(StakingError::Overflow)?
        {
            let deadline = clock
                .epoch_start_timestamp
                .checked_add(i64::from(config.settlement_window_seconds))
                .ok_or(StakingError::Overflow)?;
            if clock.unix_timestamp < deadline {
                return Err(StakingError::SettlementNotExpired.into());
            }
        }
        while !iter.iterator_is_empty() {
            let batch_account = iter.next_mut("unused_batch")?;
            let batch_rent_payer = iter.next_mut("batch_rent_payer")?;
            let batch = state::load_batch(batch_account)?;
            if batch.epoch != *epoch_account.address() {
                return Err(StakingError::InvalidEpoch.into());
            }
            check_key(batch_rent_payer, &batch.rent_payer)?;
            let mut chain =
                state::read_chain(&epoch_account.try_borrow()?, batch.hash_chain_index)?;
            if chain.executed || !chain.batch_exists {
                return Err(StakingError::InvalidChain.into());
            }
            close_to(batch_account, batch_rent_payer)?;
            chain.batch_exists = false;
            state::write_chain(
                &mut epoch_account.try_borrow_mut()?,
                batch.hash_chain_index,
                &chain,
            )?;
        }
    }
    if header.resolved_intent_count != header.intent_count {
        return if has_cleanup {
            Ok(())
        } else {
            Err(StakingError::UnresolvedIntents.into())
        };
    }
    for index in 0..M {
        let index = u16::try_from(index).map_err(|_| StakingError::Overflow)?;
        if state::read_chain(&epoch_account.try_borrow()?, index)?.batch_exists {
            return if has_cleanup {
                Ok(())
            } else {
                Err(StakingError::OutstandingBatches.into())
            };
        }
    }
    if epoch_account.lamports() < header.fee_balance_lamports {
        return Err(StakingError::InvalidAccount.into());
    }
    if header.fee_balance_lamports != 0 {
        let balance = crank
            .lamports()
            .checked_add(header.fee_balance_lamports)
            .ok_or(StakingError::Overflow)?;
        crank.set_lamports(balance);
        epoch_account.set_lamports(
            epoch_account
                .lamports()
                .checked_sub(header.fee_balance_lamports)
                .ok_or(StakingError::Overflow)?,
        );
    }
    close_to(epoch_account, rent_payer)
}

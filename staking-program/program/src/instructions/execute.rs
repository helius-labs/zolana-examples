use pinocchio::{
    cpi::{invoke_signed_with_bounds, Seed, Signer},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
    AccountView, Address, ProgramResult,
};
use wincode::{SchemaRead, SchemaWrite};
use zolana_account_checks::AccountIterator;
use zolana_interface::{
    instruction::{
        instruction_data::transact::{InputUtxo, TransactOutput},
        tag, CreateCacheData,
    },
    NullifierPda, NULLIFIER_PDA_SIZE, PROGRAM_ID_PUBKEY, SHIELDED_POOL_CPI_AUTHORITY_PUBKEY,
    SOL_INTERFACE_PUBKEY,
};
use zolana_program::cpi::SppTransactAccounts;

use super::{
    management::check_batch_chain,
    proof::{self, LogicProof},
    shared::{check_key, check_system, close_to, no_more, parse},
};
use crate::{
    error::StakingError,
    math::PoolSnapshot,
    pda, pool,
    settlement::{self, CachedUtxo, LegContext, PoolOperation, SettlementAmounts, SettlementLegs},
    state::{self, BatchEntry, BatchHeader, Config},
};
use spl_stake_pool::state::StakePool;

/// Sysvar and program ids the stake pool's `WithdrawSol` reads.
pub use pinocchio::sysvars::clock::CLOCK_ID;

pub mod stake_history {
    pinocchio::address::declare_id!("SysvarStakeHistory1111111111111111111111111");
}

pub mod stake_program {
    pinocchio::address::declare_id!("Stake11111111111111111111111111111111111111");
}

/// Account bound for the withdrawal transact CPI: 4 fixed + 1 input tree +
/// up to 16 nullifier PDAs + 1 owner + 7 interface-transfer accounts + 2 cache
/// = 31.
const WITHDRAWAL_MAX_ACCOUNTS: usize = 31;
/// Payout transact CPI: 4 fixed + 1 input tree + 1 nullifier PDA + 1 owner +
/// 7 interface-transfer accounts + 1 cache = 15.
const PAYOUT_MAX_ACCOUNTS: usize = 15;

#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ExecuteBatchIxData {
    pub settlement_proof: LogicProof,
    /// `None` iff every stored entry is cancelled.
    pub legs: Option<SettlementLegs>,
}

/// The outer accounts both SPP legs forward, generic over the account type so
/// the CPI layouts are testable off-chain.
#[derive(Clone, Debug)]
pub(crate) struct SppLegAccounts<T> {
    pub payer: T,
    pub escrow_tree: T,
    pub spp_program: T,
    pub system_program: T,
    pub escrow_authority: T,
    pub sol_vault: T,
    pub token_vault: T,
    pub sol_interface: T,
    pub spl_interface: T,
    pub spp_cpi_authority: T,
    pub pool_mint: T,
    pub token_program: T,
    pub cache: T,
    pub cached_nullifier_pda: T,
}

impl<T: Clone> SppLegAccounts<T> {
    fn prefix(&self) -> Vec<T> {
        vec![
            self.payer.clone(),
            self.escrow_tree.clone(),
            self.spp_program.clone(),
            self.system_program.clone(),
            // The escrow tree is also the only input tree (tree context 0).
            self.escrow_tree.clone(),
        ]
    }

    /// Transact layout of the withdrawal leg: the active escrows' nullifier
    /// PDAs, `escrow_authority` as the owner signer, the SOL and SPL withdrawal
    /// groups (each omitted when zero, matching the ix data), then the write
    /// cache and its writer.
    pub fn withdrawal(&self, nullifier_pdas: &[T], amounts: &SettlementAmounts) -> Vec<T> {
        let mut list = self.prefix();
        list.extend_from_slice(nullifier_pdas);
        list.push(self.escrow_authority.clone());
        if amounts.escrow_sol != 0 {
            list.push(self.sol_interface.clone());
            list.push(self.sol_vault.clone());
        }
        if amounts.escrow_pool_tokens != 0 {
            list.push(self.spp_cpi_authority.clone());
            list.push(self.pool_mint.clone());
            list.push(self.spl_interface.clone());
            list.push(self.token_vault.clone());
            list.push(self.token_program.clone());
        }
        list.push(self.cache.clone());
        list.push(self.escrow_authority.clone());
        list
    }

    /// Transact layout of the payout leg: the cached input's nullifier PDA,
    /// `sol_vault` as the owner signer, the SOL and SPL deposit groups (each
    /// omitted when zero), then the read cache.
    pub fn payout(&self, amounts: &SettlementAmounts) -> Vec<T> {
        let mut list = self.prefix();
        list.push(self.cached_nullifier_pda.clone());
        list.push(self.sol_vault.clone());
        if amounts.payout_sol != 0 {
            list.push(self.sol_interface.clone());
            list.push(self.sol_vault.clone());
        }
        if amounts.payout_pool_tokens != 0 {
            list.push(self.pool_mint.clone());
            list.push(self.spl_interface.clone());
            list.push(self.sol_vault.clone());
            list.push(self.token_vault.clone());
            list.push(self.token_program.clone());
        }
        list.push(self.cache.clone());
        list
    }
}

/// `create_cache` instruction data: tag followed by the wincode payload
/// `write_authority (32) || nonce u64 LE || tree_id u16 LE || expires_at i64 LE`.
pub fn create_cache_data(data: &CreateCacheData) -> Result<Vec<u8>, StakingError> {
    let payload = wincode::serialize(data).map_err(|_| StakingError::InvalidInstructionData)?;
    let mut bytes = Vec::with_capacity(1 + payload.len());
    bytes.push(tag::CREATE_CACHE);
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// A cancelled entry's nullifier was queued by SPP: its PDA exists, belongs
/// to SPP, and records a queue index on the escrow tree.
fn check_cancelled(account: &AccountView, expected: &Address, tree_id: u16) -> ProgramResult {
    let error = StakingError::InvalidCancellation;
    if account.address() != expected
        || !account.owned_by(&PROGRAM_ID_PUBKEY)
        || account.data_len() != NULLIFIER_PDA_SIZE
    {
        return Err(error.into());
    }
    let record = NullifierPda::read_from(&account.try_borrow()?).ok_or(error)?;
    if record.queue_index == 0 || record.tree_id != tree_id {
        return Err(error.into());
    }
    Ok(())
}

fn invoke_create_cache(
    payer: &AccountView,
    cache: &AccountView,
    system_program: &AccountView,
    data: &CreateCacheData,
) -> ProgramResult {
    let bytes = create_cache_data(data)?;
    let metas = [
        InstructionAccount::writable_signer(payer.address()),
        InstructionAccount::writable(cache.address()),
        InstructionAccount::readonly(system_program.address()),
    ];
    let view = InstructionView {
        program_id: &PROGRAM_ID_PUBKEY,
        accounts: &metas,
        data: &bytes,
    };
    invoke_signed_with_bounds::<3, _>(&view, &[*payer, *cache, *system_program], &[])
}

fn invoke_close_cache(
    cache: &AccountView,
    rent_recipient: &AccountView,
    writer: &AccountView,
    writer_signer: Signer,
) -> ProgramResult {
    let metas = [
        InstructionAccount::writable(cache.address()),
        InstructionAccount::writable(rent_recipient.address()),
        InstructionAccount::readonly_signer(writer.address()),
    ];
    let data = [tag::CLOSE_CACHE];
    let view = InstructionView {
        program_id: &PROGRAM_ID_PUBKEY,
        accounts: &metas,
        data: &data,
    };
    invoke_signed_with_bounds::<3, _>(&view, &[*cache, *rent_recipient, *writer], &[writer_signer])
}

/// The always-present stake pool section.
struct PoolSection<'a> {
    program: &'a AccountView,
    stake_pool: &'a AccountView,
    withdraw_authority: &'a AccountView,
    reserve_stake: &'a AccountView,
    manager_fee_account: &'a AccountView,
    sysvar_clock: &'a AccountView,
    sysvar_stake_history: &'a AccountView,
    stake_program: &'a AccountView,
}

/// Values every settlement CPI shares.
struct SettleContext<'a> {
    config_key: &'a Address,
    config: &'a Config,
    batch: &'a BatchHeader,
    settlement_deadline: i64,
    tree_id: u16,
    authority_key: &'a Address,
    authority_bump: u8,
    vaults: &'a pool::Vaults,
}

pub fn process_execute_batch(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let ix: Box<ExecuteBatchIxData> = Box::new(parse(data)?);
    let mut iter = AccountIterator::new(accounts);
    let payer = iter.next_signer_mut("payer")?;
    let config_account = iter.next_account("config")?;
    let epoch_account = iter.next_mut("epoch")?;
    let batch_account = iter.next_mut("batch")?;
    let escrow_authority = iter.next_account("escrow_authority")?;
    let sol_vault = iter.next_mut("sol_vault")?;
    let token_vault = iter.next_mut("token_vault")?;
    let crank = iter.next_mut("crank")?;
    let rent_payer = iter.next_mut("rent_payer")?;
    let escrow_tree = iter.next_mut("escrow_tree")?;

    // State and relations.
    let config_key = *config_account.address();
    let config = load_config_boxed(config_account)?;
    let mut header = state::load_epoch(epoch_account)?;
    if header.config != config_key {
        return Err(StakingError::InvalidConfig.into());
    }
    let batch = load_batch_boxed(batch_account)?;
    check_key(epoch_account, &batch.epoch)?;
    check_batch_chain(epoch_account, &header, &batch)?;
    if batch.filled_count != batch.intent_count {
        return Err(StakingError::IncompleteBatch.into());
    }

    // Fixed account identities.
    let (authority_key, authority_bump) = pda::escrow_authority(&config_key);
    check_key(escrow_authority, &authority_key)?;
    let vaults = pool::Vaults::derive(&config_key, &config);
    check_key(sol_vault, &vaults.sol_vault)?;
    let tokens_before = pool::vault_tokens(token_vault, &vaults, &config)?;
    if crank.address() != &batch.crank.signing_pubkey
        || crank.address() == epoch_account.address()
        || crank.address() == batch_account.address()
    {
        return Err(StakingError::InvalidCrank.into());
    }
    check_key(rent_payer, &batch.rent_payer)?;
    if escrow_tree.address() != &config.escrow_tree {
        return Err(StakingError::InvalidTree.into());
    }

    // Stored entries.
    let entries = state::read_entries(&batch_account.try_borrow()?)?;
    let active = entries
        .iter()
        .filter(|(_, output)| output.is_some())
        .count();
    if ix.legs.is_some() != (active > 0) {
        return Err(StakingError::InvalidInstructionData.into());
    }
    if entries.iter().any(|(input, _)| input.tree_index != 0) {
        return Err(StakingError::InvalidTree.into());
    }
    let tree_id = zolana_program::compression::load_tree_id(escrow_tree)
        .map_err(|_| StakingError::InvalidTree)?;

    // One nullifier PDA per stored entry, in entry order.
    let mut active_pdas = Vec::with_capacity(active);
    for (input, output) in &entries {
        let account = iter.next_account("nullifier_pda")?;
        check_entry_pda(
            account,
            &config.escrow_tree,
            &input.nullifier_hash,
            output.is_some(),
            tree_id,
        )?;
        if output.is_some() {
            active_pdas.push(*account);
        }
    }

    // SPP section, present only when an entry settles.
    let spp = if active > 0 {
        let fixed = SppFixedAccounts {
            payer,
            escrow_tree,
            escrow_authority,
            sol_vault,
            token_vault,
        };
        Some(parse_spp_section(&mut iter, &fixed, &config, &batch)?)
    } else {
        None
    };

    // Pool section.
    let pool_section = PoolSection {
        program: iter.next_account("stake_pool_program")?,
        stake_pool: iter.next_mut("stake_pool")?,
        withdraw_authority: iter.next_account("pool_withdraw_authority")?,
        reserve_stake: iter.next_mut("reserve_stake")?,
        manager_fee_account: iter.next_mut("manager_fee_account")?,
        sysvar_clock: iter.next_account("sysvar_clock")?,
        sysvar_stake_history: iter.next_account("sysvar_stake_history")?,
        stake_program: iter.next_account("stake_program")?,
    };
    no_more(&iter)?;
    let pool = load_pool_section(&vaults, &config, &pool_section)?;
    let snapshot = pool::snapshot(&pool);
    let operation = match &ix.legs {
        Some(legs) => legs
            .amounts
            .pool_operation(&snapshot)
            .map_err(|_| StakingError::InvalidPool)?,
        None => PoolOperation::None,
    };

    // Cached UTXO and the settlement proof.
    let cached = match (
        &ix.legs,
        entries.iter().find(|(_, output)| output.is_some()),
    ) {
        (Some(legs), Some((first, _))) => Some(settlement::cached_utxo(
            &vaults.sol_vault,
            &first.nullifier_hash,
            &legs.withdrawal_output_seed,
            tree_id,
        )?),
        _ => None,
    };
    if let (Some((spp_accounts, _)), Some(cached)) = (&spp, &cached) {
        check_key(
            &spp_accounts.cached_nullifier_pda,
            &zolana_interface::pda::nullifier_pda(&config.escrow_tree, &cached.nullifier).0,
        )?;
    }
    verify_settlement_proof(
        &ix,
        &config_key,
        &config,
        epoch_account.address(),
        &batch,
        &entries,
        tree_id,
        &snapshot,
        cached.as_ref(),
    )?;

    // Settlement CPIs. Epoch and batch are never passed; no borrow is held.
    if let (Some(legs), Some(cached), Some((spp_accounts, spl_interface_bump))) =
        (&ix.legs, &cached, &spp)
    {
        let context = SettleContext {
            config_key: &config_key,
            config: &config,
            batch: &batch,
            settlement_deadline: header.settlement_deadline,
            tree_id,
            authority_key: &authority_key,
            authority_bump,
            vaults: &vaults,
        };
        let sol_before = sol_vault.lamports();
        settle(
            &context,
            legs,
            cached,
            spp_accounts,
            *spl_interface_bump,
            &entries,
            &active_pdas,
            &pool,
            operation,
            &pool_section,
        )?;
        if sol_vault.lamports() != sol_before
            || pool::vault_tokens(token_vault, &vaults, &config)? != tokens_before
        {
            return Err(StakingError::VaultBalanceMismatch.into());
        }
    }

    // Intent fees for the settled entries go to the crank.
    let fees = u64::try_from(active)
        .map_err(|_| StakingError::Overflow)?
        .checked_mul(config.intent_fee_lamports)
        .ok_or(StakingError::Overflow)?;
    if fees != 0 {
        header.fee_balance_lamports = header
            .fee_balance_lamports
            .checked_sub(fees)
            .ok_or(StakingError::InvalidAccount)?;
        let epoch_lamports = epoch_account
            .lamports()
            .checked_sub(fees)
            .ok_or(StakingError::InvalidAccount)?;
        epoch_account.set_lamports(epoch_lamports);
        let crank_lamports = crank
            .lamports()
            .checked_add(fees)
            .ok_or(StakingError::Overflow)?;
        crank.set_lamports(crank_lamports);
    }

    header.resolve(u32::try_from(active).map_err(|_| StakingError::Overflow)?)?;
    let mut chain = state::read_chain(&epoch_account.try_borrow()?, batch.hash_chain_index)?;
    chain.executed = true;
    chain.batch_exists = false;
    {
        let mut data = epoch_account.try_borrow_mut()?;
        state::write_chain(&mut data, batch.hash_chain_index, &chain)?;
        state::write_epoch(&mut data, &header)?;
    }
    close_to(batch_account, rent_payer)
}

#[inline(never)]
fn load_config_boxed(account: &AccountView) -> Result<Box<Config>, ProgramError> {
    Ok(Box::new(state::load_config(account)?))
}

#[inline(never)]
fn load_batch_boxed(account: &AccountView) -> Result<Box<BatchHeader>, ProgramError> {
    Ok(Box::new(state::load_batch(account)?))
}

/// Active entries: the writable PDA SPP creates on spend. Cancelled entries:
/// the PDA SPP created when the cancel spent the escrow.
#[inline(never)]
fn check_entry_pda(
    account: &AccountView,
    tree: &Address,
    nullifier: &[u8; 32],
    active: bool,
    tree_id: u16,
) -> ProgramResult {
    let expected = zolana_interface::pda::nullifier_pda(tree, nullifier).0;
    if !active {
        return check_cancelled(account, &expected, tree_id);
    }
    if account.address() != &expected || !account.is_writable() {
        return Err(StakingError::InvalidAccount.into());
    }
    Ok(())
}

/// Fixed accounts the SPP legs forward alongside the SPP section.
struct SppFixedAccounts<'a> {
    payer: &'a AccountView,
    escrow_tree: &'a AccountView,
    escrow_authority: &'a AccountView,
    sol_vault: &'a AccountView,
    token_vault: &'a AccountView,
}

/// Parses and checks the SPP section. Returns the CPI accounts and the SPL
/// interface bump.
#[inline(never)]
fn parse_spp_section(
    iter: &mut AccountIterator<'_>,
    fixed: &SppFixedAccounts,
    config: &Config,
    batch: &BatchHeader,
) -> Result<(SppLegAccounts<AccountView>, u8), ProgramError> {
    let cache = iter.next_mut("cache")?;
    let cached_nullifier_pda = iter.next_mut("cached_nullifier_pda")?;
    let spp_program = iter.next_account("spp_program")?;
    let system_program = iter.next_account("system_program")?;
    let sol_interface = iter.next_mut("sol_interface")?;
    let spl_interface = iter.next_mut("spl_interface")?;
    let spp_cpi_authority = iter.next_non_mut("spp_cpi_authority")?;
    let pool_mint = iter.next_mut("pool_mint")?;
    let token_program = iter.next_account("token_program")?;
    check_key(
        cache,
        &zolana_interface::pda::cache(fixed.payer.address(), u64::from(batch.hash_chain_index)).0,
    )?;
    check_key(spp_program, &PROGRAM_ID_PUBKEY)?;
    check_system(system_program)?;
    check_key(sol_interface, &SOL_INTERFACE_PUBKEY)?;
    let (spl_key, spl_interface_bump) =
        zolana_interface::pda::spl_interface_with_bump(&config.pool_mint);
    check_key(spl_interface, &spl_key)?;
    check_key(spp_cpi_authority, &SHIELDED_POOL_CPI_AUTHORITY_PUBKEY)?;
    check_key(pool_mint, &config.pool_mint)?;
    check_key(token_program, &config.token_program_id)?;
    let accounts = SppLegAccounts {
        payer: *fixed.payer,
        escrow_tree: *fixed.escrow_tree,
        spp_program: *spp_program,
        system_program: *system_program,
        escrow_authority: *fixed.escrow_authority,
        sol_vault: *fixed.sol_vault,
        token_vault: *fixed.token_vault,
        sol_interface: *sol_interface,
        spl_interface: *spl_interface,
        spp_cpi_authority: *spp_cpi_authority,
        pool_mint: *pool_mint,
        token_program: *token_program,
        cache: *cache,
        cached_nullifier_pda: *cached_nullifier_pda,
    };
    Ok((accounts, spl_interface_bump))
}

#[inline(never)]
fn load_pool_section(
    vaults: &pool::Vaults,
    config: &Config,
    section: &PoolSection,
) -> Result<Box<StakePool>, ProgramError> {
    if section.program.address() != &config.stake_pool_program_id || !section.program.executable() {
        return Err(StakingError::InvalidPool.into());
    }
    check_key(section.sysvar_clock, &CLOCK_ID)?;
    check_key(section.sysvar_stake_history, &stake_history::ID)?;
    check_key(section.stake_program, &stake_program::ID)?;
    let pool = Box::new(pool::load_pool(section.stake_pool, config)?);
    pool::validate_pool_accounts(
        vaults,
        config,
        &pool,
        section.reserve_stake,
        section.withdraw_authority,
        section.manager_fee_account,
    )?;
    Ok(pool)
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn verify_settlement_proof(
    ix: &ExecuteBatchIxData,
    config_key: &Address,
    config: &Config,
    epoch: &Address,
    batch: &BatchHeader,
    entries: &[BatchEntry],
    tree_id: u16,
    snapshot: &PoolSnapshot,
    cached: Option<&CachedUtxo>,
) -> ProgramResult {
    let context = settlement::proof_context(
        config_key,
        config,
        epoch,
        batch,
        entries,
        tree_id,
        snapshot,
        ix.legs.as_ref().zip(cached),
    )?;
    proof::verify(
        &ix.settlement_proof,
        context.hash()?,
        &crate::verifying_keys::settlement::VERIFYINGKEY,
    )
}

/// Cache, withdrawal leg, pool operation, payout leg, cache close.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn settle(
    context: &SettleContext,
    legs: &SettlementLegs,
    cached: &CachedUtxo,
    accounts: &SppLegAccounts<AccountView>,
    spl_interface_bump: u8,
    entries: &[BatchEntry],
    active_pdas: &[AccountView],
    pool: &StakePool,
    operation: PoolOperation,
    pool_section: &PoolSection,
) -> ProgramResult {
    invoke_create_cache(
        &accounts.payer,
        &accounts.cache,
        &accounts.system_program,
        &CreateCacheData {
            write_authority: *context.authority_key,
            nonce: u64::from(context.batch.hash_chain_index),
            tree_id: context.tree_id,
            expires_at: context.settlement_deadline,
        },
    )?;
    let leg_context = LegContext {
        expiry_unix_ts: u64::try_from(context.settlement_deadline)
            .map_err(|_| StakingError::Overflow)?,
        tx_viewing_pk: context.batch.tx_viewing_pk,
        salt: context.batch.salt,
        spl_interface_bump,
        sol_vault: &context.vaults.sol_vault,
    };
    let authority_bump = [context.authority_bump];
    let authority_seeds = [
        Seed::from(pda::ESCROW_AUTHORITY_SEED),
        Seed::from(context.config_key.as_ref()),
        Seed::from(&authority_bump),
    ];
    invoke_withdrawal(
        &leg_context,
        legs,
        cached,
        accounts,
        entries,
        active_pdas,
        context.authority_key,
        Signer::from(&authority_seeds),
    )?;
    let pool_accounts = [
        *pool_section.program,
        *pool_section.stake_pool,
        *pool_section.withdraw_authority,
        *pool_section.reserve_stake,
        *pool_section.manager_fee_account,
        *pool_section.sysvar_clock,
        *pool_section.sysvar_stake_history,
        *pool_section.stake_program,
        accounts.sol_vault,
        accounts.token_vault,
        accounts.pool_mint,
        accounts.token_program,
        accounts.system_program,
    ];
    pool::invoke_operation(
        context.config_key,
        context.vaults,
        context.config,
        pool,
        operation,
        &pool_accounts,
    )?;
    let vault_bump = [context.vaults.sol_vault_bump];
    let vault_seeds = [
        Seed::from(pda::SOL_VAULT_SEED),
        Seed::from(context.config_key.as_ref()),
        Seed::from(&vault_bump),
    ];
    invoke_payout(
        &leg_context,
        legs,
        cached,
        accounts,
        entries,
        &context.vaults.sol_vault,
        Signer::from(&vault_seeds),
    )?;
    invoke_close_cache(
        &accounts.cache,
        &accounts.payer,
        &accounts.escrow_authority,
        Signer::from(&authority_seeds),
    )
}

/// Spends the active escrows into the vaults and writes the cached UTXO.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn invoke_withdrawal(
    leg_context: &LegContext,
    legs: &SettlementLegs,
    cached: &CachedUtxo,
    accounts: &SppLegAccounts<AccountView>,
    entries: &[BatchEntry],
    active_pdas: &[AccountView],
    authority_key: &Address,
    signer: Signer,
) -> ProgramResult {
    let active_inputs: Vec<InputUtxo> = entries
        .iter()
        .filter(|(_, output)| output.is_some())
        .map(|(input, _)| *input)
        .collect();
    let bytes = settlement::withdrawal_ix_data(
        leg_context,
        &active_inputs,
        cached,
        &legs.amounts,
        legs.escrow_tree_context,
        &legs.spend,
    )?
    .serialize()
    .map_err(|_| StakingError::InvalidInstructionData)?;
    let list = accounts.withdrawal(active_pdas, &legs.amounts);
    let signer_pdas = [authority_key];
    SppTransactAccounts::new(&list, &signer_pdas)?
        .invoke::<WITHDRAWAL_MAX_ACCOUNTS>(&bytes, &[signer])
}

/// Spends the cached UTXO into the stored payouts, funded by the vaults.
#[inline(never)]
fn invoke_payout(
    leg_context: &LegContext,
    legs: &SettlementLegs,
    cached: &CachedUtxo,
    accounts: &SppLegAccounts<AccountView>,
    entries: &[BatchEntry],
    vault_key: &Address,
    signer: Signer,
) -> ProgramResult {
    let outputs: Vec<TransactOutput> = entries
        .iter()
        .filter_map(|(_, output)| output.clone())
        .collect();
    let bytes = settlement::payout_ix_data(
        leg_context,
        outputs,
        cached,
        &legs.amounts,
        legs.payout_nullifier_root_index,
        &legs.payout,
    )?
    .serialize()
    .map_err(|_| StakingError::InvalidInstructionData)?;
    let list = accounts.payout(&legs.amounts);
    let signer_pdas = [vault_key];
    SppTransactAccounts::new(&list, &signer_pdas)?.invoke::<PAYOUT_MAX_ACCOUNTS>(&bytes, &[signer])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> SppLegAccounts<&'static str> {
        SppLegAccounts {
            payer: "payer",
            escrow_tree: "escrow_tree",
            spp_program: "spp_program",
            system_program: "system_program",
            escrow_authority: "escrow_authority",
            sol_vault: "sol_vault",
            token_vault: "token_vault",
            sol_interface: "sol_interface",
            spl_interface: "spl_interface",
            spp_cpi_authority: "spp_cpi_authority",
            pool_mint: "pool_mint",
            token_program: "token_program",
            cache: "cache",
            cached_nullifier_pda: "cached_nullifier_pda",
        }
    }

    fn amounts(
        escrow_sol: u64,
        escrow_pool_tokens: u64,
        payout_sol: u64,
        payout_pool_tokens: u64,
    ) -> SettlementAmounts {
        SettlementAmounts {
            escrow_sol,
            escrow_pool_tokens,
            payout_sol,
            payout_pool_tokens,
        }
    }

    #[test]
    fn withdrawal_layout_follows_the_transact_order() {
        let accounts = labels();
        let pdas = ["nf0", "nf1"];
        assert_eq!(
            accounts.withdrawal(&pdas, &amounts(5, 7, 0, 0)),
            vec![
                "payer",
                "escrow_tree",
                "spp_program",
                "system_program",
                "escrow_tree",
                "nf0",
                "nf1",
                "escrow_authority",
                "sol_interface",
                "sol_vault",
                "spp_cpi_authority",
                "pool_mint",
                "spl_interface",
                "token_vault",
                "token_program",
                "cache",
                "escrow_authority",
            ]
        );
        assert_eq!(
            accounts.withdrawal(&pdas[..1], &amounts(0, 7, 0, 0)),
            vec![
                "payer",
                "escrow_tree",
                "spp_program",
                "system_program",
                "escrow_tree",
                "nf0",
                "escrow_authority",
                "spp_cpi_authority",
                "pool_mint",
                "spl_interface",
                "token_vault",
                "token_program",
                "cache",
                "escrow_authority",
            ]
        );
        let full = [""; crate::N];
        assert_eq!(
            accounts.withdrawal(&full, &amounts(1, 1, 0, 0)).len(),
            WITHDRAWAL_MAX_ACCOUNTS
        );
    }

    #[test]
    fn payout_layout_follows_the_transact_order() {
        let accounts = labels();
        assert_eq!(
            accounts.payout(&amounts(0, 0, 3, 4)),
            vec![
                "payer",
                "escrow_tree",
                "spp_program",
                "system_program",
                "escrow_tree",
                "cached_nullifier_pda",
                "sol_vault",
                "sol_interface",
                "sol_vault",
                "pool_mint",
                "spl_interface",
                "sol_vault",
                "token_vault",
                "token_program",
                "cache",
            ]
        );
        assert_eq!(
            accounts.payout(&amounts(0, 0, 3, 0)),
            vec![
                "payer",
                "escrow_tree",
                "spp_program",
                "system_program",
                "escrow_tree",
                "cached_nullifier_pda",
                "sol_vault",
                "sol_interface",
                "sol_vault",
                "cache",
            ]
        );
        assert_eq!(
            accounts.payout(&amounts(0, 0, 3, 4)).len(),
            PAYOUT_MAX_ACCOUNTS
        );
    }

    #[test]
    fn create_cache_data_is_tag_then_fixed_le_fields() {
        let data = CreateCacheData {
            write_authority: Address::new_from_array([7; 32]),
            nonce: 0x0102,
            tree_id: 0x0304,
            expires_at: -2,
        };
        let mut expected = vec![tag::CREATE_CACHE];
        expected.extend_from_slice(&[7; 32]);
        expected.extend_from_slice(&0x0102u64.to_le_bytes());
        expected.extend_from_slice(&0x0304u16.to_le_bytes());
        expected.extend_from_slice(&(-2i64).to_le_bytes());
        assert_eq!(create_cache_data(&data), Ok(expected));
        assert_eq!(tag::CREATE_CACHE, 22);
        assert_eq!(tag::CLOSE_CACHE, 23);
    }

    #[test]
    fn stake_pool_withdrawal_reads_the_declared_sysvars() {
        let pool = Address::new_from_array([1; 32]);
        let other = Address::new_from_array([2; 32]);
        let ix = spl_stake_pool::instruction::withdraw_sol(
            &spl_stake_pool::id(),
            &pool,
            &other,
            &other,
            &other,
            &other,
            &other,
            &other,
            &other,
            &other,
            1,
        );
        let keys: Vec<Address> = ix.accounts.iter().map(|meta| meta.pubkey).collect();
        assert!(keys.contains(&CLOCK_ID));
        assert!(keys.contains(&stake_history::ID));
        assert!(keys.contains(&stake_program::ID));
    }

    #[test]
    fn execute_batch_ix_data_round_trips() {
        let settlement_proof = LogicProof {
            a: [1; 32],
            b: [2; 64],
            c: [3; 32],
            commitment: [4; 32],
            commitment_pok: [5; 32],
        };
        let spp_proof = |byte: u8| settlement::SppProofData {
            proof: zolana_interface::instruction::instruction_data::transact::TransactProof {
                a: [byte; 32],
                b: [byte; 128],
                c: [byte; 32],
            },
            private_tx_hash: [byte.wrapping_add(1); 32],
        };
        let legs = SettlementLegs {
            amounts: amounts(6, 7, 8, 9),
            escrow_tree_context:
                zolana_interface::instruction::instruction_data::transact::TreeContext {
                    utxo_tree_root_index: 10,
                    nullifier_tree_root_index: 11,
                },
            payout_nullifier_root_index: 12,
            withdrawal_output_seed: [13; 32],
            spend: spp_proof(14),
            payout: spp_proof(16),
        };
        for legs in [None, Some(legs)] {
            let ix = ExecuteBatchIxData {
                settlement_proof,
                legs,
            };
            let bytes = wincode::serialize(&ix).unwrap();
            assert_eq!(parse::<ExecuteBatchIxData>(&bytes), Ok(ix));
        }
    }
}

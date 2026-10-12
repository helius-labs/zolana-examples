use borsh::BorshDeserialize;
use pinocchio::{
    cpi::{invoke_signed_with_bounds, Seed, Signer},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
    AccountView, Address, ProgramResult,
};
use spl_stake_pool::state::StakePool;
use spl_token_2022_interface::{
    extension::{BaseStateWithExtensions, StateWithExtensions},
    state::{Account as TokenAccount, AccountState},
};

use crate::{
    error::StakingError, math::PoolSnapshot, pda, settlement::PoolOperation, state::Config,
};

mod associated_token {
    pinocchio::address::declare_id!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
}

pub fn token_vault(config_key: &Address, config: &Config) -> Address {
    token_vault_of(&pda::sol_vault(config_key).0, config)
}

/// The `sol_vault`'s pool-mint ATA.
fn token_vault_of(sol_vault: &Address, config: &Config) -> Address {
    Address::find_program_address(
        &[
            sol_vault.as_ref(),
            config.token_program_id.as_ref(),
            config.pool_mint.as_ref(),
        ],
        &associated_token::ID,
    )
    .0
}

/// The config's vault addresses, derived once per instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vaults {
    pub sol_vault: Address,
    pub sol_vault_bump: u8,
    pub token_vault: Address,
}

impl Vaults {
    pub fn derive(config_key: &Address, config: &Config) -> Self {
        let (sol_vault, sol_vault_bump) = pda::sol_vault(config_key);
        Self {
            token_vault: token_vault_of(&sol_vault, config),
            sol_vault,
            sol_vault_bump,
        }
    }
}

pub fn load_pool(account: &AccountView, config: &Config) -> Result<StakePool, ProgramError> {
    if config.stake_pool_program_id != spl_stake_pool::id()
        || account.address() != &config.stake_pool
        || !account.owned_by(&config.stake_pool_program_id)
    {
        return Err(StakingError::InvalidPool.into());
    }
    let pool = StakePool::deserialize(&mut account.try_borrow()?.as_ref())
        .map_err(|_| StakingError::InvalidPool)?;
    if !pool.is_valid()
        || pool.pool_mint != config.pool_mint
        || pool.token_program_id != config.token_program_id
    {
        return Err(StakingError::InvalidPool.into());
    }
    Ok(pool)
}

pub fn snapshot(pool: &StakePool) -> PoolSnapshot {
    PoolSnapshot {
        total_lamports: pool.total_lamports,
        pool_token_supply: pool.pool_token_supply,
        deposit_fee: pool.sol_deposit_fee,
        withdrawal_fee: pool.sol_withdrawal_fee,
    }
}

pub fn vault_tokens(
    vault: &AccountView,
    vaults: &Vaults,
    config: &Config,
) -> Result<u64, ProgramError> {
    if vault.address() != &vaults.token_vault || !vault.owned_by(&config.token_program_id) {
        return Err(StakingError::InvalidAccount.into());
    }
    let data = vault.try_borrow()?;
    let token = StateWithExtensions::<TokenAccount>::unpack(&data)
        .map_err(|_| StakingError::InvalidAccount)?;
    if token.base.mint != config.pool_mint || token.base.owner != vaults.sol_vault {
        return Err(StakingError::InvalidAccount.into());
    }
    Ok(token.base.amount)
}

/// Validate the public pool accounts before reading a quote or invoking the
/// unmodified program. The manager fee destination must not alias our vault.
pub fn validate_pool_accounts(
    vaults: &Vaults,
    config: &Config,
    pool: &StakePool,
    reserve: &AccountView,
    withdraw_authority: &AccountView,
    fee_account: &AccountView,
) -> ProgramResult {
    let authority = spl_stake_pool::find_withdraw_authority_program_address(
        &config.stake_pool_program_id,
        &config.stake_pool,
    )
    .0;
    if reserve.address() != &pool.reserve_stake
        || withdraw_authority.address() != &authority
        || fee_account.address() != &pool.manager_fee_account
        || pool.manager_fee_account == vaults.token_vault
    {
        return Err(StakingError::InvalidPool.into());
    }
    if !fee_account.owned_by(&config.token_program_id) {
        return Err(StakingError::InvalidPool.into());
    }
    let fee_data = fee_account.try_borrow()?;
    let fee = StateWithExtensions::<TokenAccount>::unpack(&fee_data)
        .map_err(|_| StakingError::InvalidPool)?;
    if fee.base.state != AccountState::Initialized || fee.base.mint != config.pool_mint {
        return Err(StakingError::InvalidPool.into());
    }
    if fee
        .get_extension_types()
        .map_err(|_| StakingError::InvalidPool)?
        .iter()
        .any(|extension| !spl_stake_pool::state::is_extension_supported_for_fee_account(extension))
    {
        return Err(StakingError::InvalidPool.into());
    }
    let vault = vaults.sol_vault;
    if pool
        .sol_deposit_authority
        .is_some_and(|address| address != vault)
        || pool
            .sol_withdraw_authority
            .is_some_and(|address| address != vault)
    {
        return Err(StakingError::InvalidPool.into());
    }
    Ok(())
}

pub fn operation_instruction(
    vaults: &Vaults,
    config: &Config,
    pool: &StakePool,
    operation: PoolOperation,
) -> Result<Option<solana_instruction::Instruction>, ProgramError> {
    let vault = vaults.sol_vault;
    let tokens = vaults.token_vault;
    let withdraw = spl_stake_pool::find_withdraw_authority_program_address(
        &config.stake_pool_program_id,
        &config.stake_pool,
    )
    .0;
    let ix = match operation {
        PoolOperation::None => return Ok(None),
        PoolOperation::DepositSol { lamports, .. } => {
            if pool.sol_deposit_authority.is_some() {
                spl_stake_pool::instruction::deposit_sol_with_authority(
                    &config.stake_pool_program_id,
                    &config.stake_pool,
                    &vault,
                    &withdraw,
                    &pool.reserve_stake,
                    &vault,
                    &tokens,
                    &pool.manager_fee_account,
                    &pool.manager_fee_account,
                    &config.pool_mint,
                    &config.token_program_id,
                    lamports,
                )
            } else {
                spl_stake_pool::instruction::deposit_sol(
                    &config.stake_pool_program_id,
                    &config.stake_pool,
                    &withdraw,
                    &pool.reserve_stake,
                    &vault,
                    &tokens,
                    &pool.manager_fee_account,
                    &pool.manager_fee_account,
                    &config.pool_mint,
                    &config.token_program_id,
                    lamports,
                )
            }
        }
        PoolOperation::WithdrawSol { pool_tokens, .. } => {
            if pool.sol_withdraw_authority.is_some() {
                spl_stake_pool::instruction::withdraw_sol_with_authority(
                    &config.stake_pool_program_id,
                    &config.stake_pool,
                    &vault,
                    &withdraw,
                    &vault,
                    &tokens,
                    &pool.reserve_stake,
                    &vault,
                    &pool.manager_fee_account,
                    &config.pool_mint,
                    &config.token_program_id,
                    pool_tokens,
                )
            } else {
                spl_stake_pool::instruction::withdraw_sol(
                    &config.stake_pool_program_id,
                    &config.stake_pool,
                    &withdraw,
                    &vault,
                    &tokens,
                    &pool.reserve_stake,
                    &vault,
                    &pool.manager_fee_account,
                    &config.pool_mint,
                    &config.token_program_id,
                    pool_tokens,
                )
            }
        }
    };
    Ok(Some(ix))
}

pub fn invoke_operation(
    config_key: &Address,
    vaults: &Vaults,
    config: &Config,
    pool: &StakePool,
    operation: PoolOperation,
    accounts: &[AccountView],
) -> ProgramResult {
    let Some(ix) = operation_instruction(vaults, config, pool, operation)? else {
        return Ok(());
    };
    if !accounts
        .iter()
        .any(|account| account.address() == &config.stake_pool_program_id && account.executable())
    {
        return Err(StakingError::InvalidPool.into());
    }
    let metas: Vec<_> = ix
        .accounts
        .iter()
        .map(|meta| InstructionAccount::new(&meta.pubkey, meta.is_writable, meta.is_signer))
        .collect();
    let views = ix
        .accounts
        .iter()
        .map(|meta| {
            accounts
                .iter()
                .find(|account| account.address() == &meta.pubkey)
                .cloned()
                .ok_or(ProgramError::NotEnoughAccountKeys)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let view = InstructionView {
        program_id: &ix.program_id,
        accounts: &metas,
        data: &ix.data,
    };
    let bump = [vaults.sol_vault_bump];
    let seeds = [
        Seed::from(pda::SOL_VAULT_SEED),
        Seed::from(config_key.as_ref()),
        Seed::from(&bump),
    ];
    invoke_signed_with_bounds::<16, _>(&view, &views, &[Signer::from(&seeds)])
}

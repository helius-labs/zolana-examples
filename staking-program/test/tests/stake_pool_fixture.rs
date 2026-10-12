//! The fabricated stake pool works with the dumped mainnet stake pool, token,
//! and stake programs, and `PoolSnapshot` predicts its SOL deposit and
//! withdrawal quotes.

mod common;

use common::{wallet, Harness, StakePoolParams, EPOCH};
use solana_address::Address;
use spl_stake_pool::state::Fee;

const USER: Address = Address::new_from_array([0x61; 32]);
const USER_TOKENS: Address = Address::new_from_array([0x62; 32]);

#[derive(Debug, PartialEq, Eq)]
struct PoolBalances {
    total_lamports: u64,
    pool_token_supply: u64,
    mint_supply: u64,
    user_lamports: u64,
    user_tokens: u64,
    manager_fee_tokens: u64,
    reserve_lamports: u64,
}

fn balances(h: &Harness, fixture: &common::StakePoolFixture) -> PoolBalances {
    let pool = h.pool_state(&fixture.pool);
    let mint = <spl_token_2022_interface::state::Mint as solana_program_pack::Pack>::unpack(
        &h.account(&fixture.mint).data,
    )
    .unwrap();
    PoolBalances {
        total_lamports: pool.total_lamports,
        pool_token_supply: pool.pool_token_supply,
        mint_supply: mint.supply,
        user_lamports: h.lamports(&USER),
        user_tokens: h.token_amount(&USER_TOKENS),
        manager_fee_tokens: h.token_amount(&fixture.manager_fee),
        reserve_lamports: h.lamports(&fixture.reserve),
    }
}

fn fee(fee: Fee, amount: u64) -> u64 {
    u64::try_from(fee.apply(amount).unwrap()).unwrap()
}

#[test]
fn deposit_and_withdraw_sol_match_the_pool_snapshot() {
    let mut h = Harness::new();
    let fixture = h.stake_pool(StakePoolParams {
        total_lamports: 1_000_000_000_000,
        pool_token_supply: 900_000_000_000,
        deposit_fee: Fee {
            denominator: 100,
            numerator: 1,
        },
        withdrawal_fee: Fee {
            denominator: 1_000,
            numerator: 3,
        },
        reserve_extra_lamports: 50_000_000_000,
        last_update_epoch: EPOCH,
    });
    h.accounts.insert(USER, wallet(20_000_000_000));
    h.token_account(USER_TOKENS, fixture.mint, USER, 0);

    let deposit = 7_000_000_000;
    let before = balances(&h, &fixture);
    let gross = fixture.snapshot.gross_deposit_tokens(deposit).unwrap();
    let minted = fixture.snapshot.deposit_tokens(deposit).unwrap();
    h.run(
        spl_stake_pool::instruction::deposit_sol(
            &spl_stake_pool::id(),
            &fixture.pool,
            &fixture.withdraw_authority,
            &fixture.reserve,
            &USER,
            &USER_TOKENS,
            &fixture.manager_fee,
            &fixture.manager_fee,
            &fixture.mint,
            &fixture.token_program,
            deposit,
        ),
        Ok(()),
    );
    let after_deposit = balances(&h, &fixture);
    assert_eq!(
        after_deposit,
        PoolBalances {
            total_lamports: before.total_lamports + deposit,
            pool_token_supply: before.pool_token_supply + gross,
            mint_supply: before.mint_supply + gross,
            user_lamports: before.user_lamports - deposit,
            user_tokens: minted,
            manager_fee_tokens: gross - minted,
            reserve_lamports: before.reserve_lamports + deposit,
        }
    );

    let snapshot = private_staking_program::math::PoolSnapshot {
        total_lamports: after_deposit.total_lamports,
        pool_token_supply: after_deposit.pool_token_supply,
        ..fixture.snapshot
    };
    let pool_tokens = minted / 2;
    let paid = snapshot.withdrawal_lamports(pool_tokens).unwrap();
    let withdrawal_fee = fee(fixture.snapshot.withdrawal_fee, pool_tokens);
    h.run(
        spl_stake_pool::instruction::withdraw_sol(
            &spl_stake_pool::id(),
            &fixture.pool,
            &fixture.withdraw_authority,
            &USER,
            &USER_TOKENS,
            &fixture.reserve,
            &USER,
            &fixture.manager_fee,
            &fixture.mint,
            &fixture.token_program,
            pool_tokens,
        ),
        Ok(()),
    );
    assert_eq!(
        balances(&h, &fixture),
        PoolBalances {
            total_lamports: after_deposit.total_lamports - paid,
            pool_token_supply: after_deposit.pool_token_supply - (pool_tokens - withdrawal_fee),
            mint_supply: after_deposit.mint_supply - (pool_tokens - withdrawal_fee),
            user_lamports: after_deposit.user_lamports + paid,
            user_tokens: minted - pool_tokens,
            manager_fee_tokens: after_deposit.manager_fee_tokens + withdrawal_fee,
            reserve_lamports: after_deposit.reserve_lamports - paid,
        }
    );
}

#[test]
fn stale_pool_rejects_deposits() {
    let mut h = Harness::new();
    let fixture = h.stake_pool(StakePoolParams {
        total_lamports: 1_000_000_000_000,
        pool_token_supply: 900_000_000_000,
        deposit_fee: Fee::default(),
        withdrawal_fee: Fee::default(),
        reserve_extra_lamports: 0,
        last_update_epoch: EPOCH - 1,
    });
    h.accounts.insert(USER, wallet(20_000_000_000));
    h.token_account(USER_TOKENS, fixture.mint, USER, 0);
    let deposit = spl_stake_pool::instruction::deposit_sol(
        &spl_stake_pool::id(),
        &fixture.pool,
        &fixture.withdraw_authority,
        &fixture.reserve,
        &USER,
        &USER_TOKENS,
        &fixture.manager_fee,
        &fixture.manager_fee,
        &fixture.mint,
        &fixture.token_program,
        1_000_000_000,
    );
    h.run(
        deposit,
        Err(solana_instruction::error::InstructionError::Custom(
            spl_stake_pool::error::StakePoolError::StakeListAndPoolOutOfDate as u32,
        )),
    );
}

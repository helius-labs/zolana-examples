//! Tested invariants:
//! 1. A fee update prices new quotes at the new fee, while a quote issued
//!    before the update fills at its stored price: the user's holdings and the
//!    maker's net balance move by exactly that quote's amounts.
//! 2. Tightening a target range triggers one automatic rebalance that brings
//!    the maker's collateral inside the new range.
//! 3. Removing a pair refuses new quotes with `PairNotServed` while a fill
//!    already issued for it still settles at its quoted amounts.
//! 4. Adding a pair fails, before anything changes, with `VaultMissing` when
//!    its vault does not exist (the vault check runs first), and with
//!    `AssetNotRegistered` when its vault exists but its shares mint is not
//!    registered in the pool.
//! 5. A removed pair can be re-added once it has retired; until then the
//!    update fails with exactly `PairExists`.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use solana_address::Address;
use zolana_program_test::fixture;

use k_lend_market_maker::{
    ConfigError, ConfigUpdate, Holdings, MakerError, MarketMaker, RangeChange, RangeUpdate,
    TargetRange,
};
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Direction, Quote},
};

use k_lend_rfq_test_utils::{
    assert::assert_maker_error,
    chain::{blocking, confirm_indexed, read_vault, wait_for_account},
    market_maker::{net_holdings, plain_deposit_out, share_account, wait_for_rebalance},
    setup::{
        create_vault, setup, TestEnv, FEE_BPS, POLL, REBALANCE_TIMEOUT, SEED_DEPOSIT, SETTLE_GRACE,
        VAULT_VISIBLE_TIMEOUT,
    },
};

const TEST_NUMBER: u16 = 19;
const SEED_COLLATERAL: u64 = 20_000_000;
const SWAP_COLLATERAL: u64 = 10_000_000;
const WITHDRAW_SHARES: u64 = 2_000_000;
const RAISED_FEE_BPS: u64 = FEE_BPS + 20;
const TIGHT_RANGE_MAX: u64 = 10_000_000;
const RETIRE_TIMEOUT: Duration = Duration::from_secs(60);

/// Invariants 1-5: runtime config updates reach new quotes, ranges and pairs
/// without touching orders already issued.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_updates_apply_to_new_quotes_ranges_and_pairs() -> Result<()> {
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        pair,
        ..
    } = setup(TEST_NUMBER).await?;
    let rpc = localnet.client.rpc();
    market_maker
        .seed_inventory(&pair, SEED_DEPOSIT, SEED_COLLATERAL)
        .await?;

    // Invariant 1: new quotes use the raised fee, the earlier quote fills at
    // its stored price.
    let rate = read_vault(rpc, &pair)?;
    let before_update = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    market_maker
        .update_config(ConfigUpdate {
            fee_bps: Some(RAISED_FEE_BPS),
            ..ConfigUpdate::default()
        })
        .await?;
    let after_update = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    assert_eq!(
        before_update.quote,
        Quote {
            direction: Direction::Deposit,
            amount_in: SWAP_COLLATERAL,
            amount_out: plain_deposit_out(&rate, SWAP_COLLATERAL, FEE_BPS)?,
        }
    );
    assert_eq!(
        after_update.quote,
        Quote {
            direction: Direction::Deposit,
            amount_in: SWAP_COLLATERAL,
            amount_out: plain_deposit_out(&rate, SWAP_COLLATERAL, RAISED_FEE_BPS)?,
        }
    );
    assert!(
        after_update.quote.amount_out < before_update.quote.amount_out,
        "amount_out at the raised fee: got {}, want below {}",
        after_update.quote.amount_out,
        before_update.quote.amount_out
    );
    // The raised fee is still active: the maker fills the earlier quote at
    // the price it stored when it issued it.
    let user_before = user.holdings(&pair)?;
    let maker_before = net_holdings(&market_maker, &pair);
    let order = user.order(&localnet.client, &pair, &before_update).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    user.complete(&localnet.client, &market_maker, &pair, &order, &fill)
        .await?;
    let user_after_swap = user.holdings(&pair)?;
    assert_eq!(
        user_after_swap,
        Holdings {
            collateral: user_before.collateral - before_update.quote.amount_in,
            shares: user_before.shares + before_update.quote.amount_out,
        }
    );
    let maker_after_swap = net_holdings(&market_maker, &pair);
    assert_eq!(
        maker_after_swap,
        Holdings {
            collateral: maker_before.collateral + before_update.quote.amount_in,
            shares: maker_before.shares - before_update.quote.amount_out,
        }
    );

    // Invariant 2: a tightened range triggers one rebalance into it.
    let tight_range = TargetRange::new(0, TIGHT_RANGE_MAX)?;
    let vault_before_rebalance = read_vault(rpc, &pair)?;
    let share_account_before = share_account(rpc, &market_maker, &pair)?;
    market_maker
        .update_config(ConfigUpdate {
            ranges: vec![RangeUpdate {
                asset: pair.token_mint,
                range: RangeChange::Set(tight_range),
            }],
            ..ConfigUpdate::default()
        })
        .await?;
    let signature = wait_for_rebalance(&market_maker, REBALANCE_TIMEOUT).await?;
    confirm_indexed(&localnet.client, signature, "rebalance")?;
    tokio::time::sleep(SETTLE_GRACE).await;
    market_maker.sync().await?;
    assert_eq!(market_maker.rebalances(), vec![signature]);
    let vault_after_rebalance = read_vault(rpc, &pair)?;
    let deposited = vault_after_rebalance.token_available - vault_before_rebalance.token_available;
    let minted = vault_after_rebalance.shares_issued - vault_before_rebalance.shares_issued;
    let holdings = net_holdings(&market_maker, &pair);
    let share_account_after = share_account(rpc, &market_maker, &pair)?;
    assert_eq!(
        holdings,
        Holdings {
            collateral: maker_after_swap.collateral - deposited,
            // The rebalance sweeps the seed's unshielded residual (up to 10 bps of `minted`)
            // and leaves its own margin in the share account.
            shares: maker_after_swap.shares + share_account_before + minted - share_account_after,
        }
    );
    assert!(
        tight_range.contains(holdings.collateral),
        "collateral after the rebalance: got {}, want within {tight_range:?}",
        holdings.collateral
    );

    // Invariant 3: a removed pair refuses quotes but settles its open fill.
    let offer = market_maker
        .quote(&pair, Direction::Withdrawal, WITHDRAW_SHARES)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    market_maker
        .update_config(ConfigUpdate {
            remove_pairs: vec![pair.vault],
            ..ConfigUpdate::default()
        })
        .await?;
    assert_maker_error(
        market_maker
            .quote(&pair, Direction::Withdrawal, WITHDRAW_SHARES)
            .await,
        "PairNotServed of the removed pair",
        |error| matches!(error, MakerError::PairNotServed { vault } if *vault == pair.vault),
    );
    let settled = user.settle(&market_maker, &pair, &order, &fill).await?;
    confirm_indexed(&localnet.client, settled, "swap")?;
    user.sync(&localnet.client).await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: user_after_swap.collateral + offer.quote.amount_out,
            shares: user_after_swap.shares - offer.quote.amount_in,
        }
    );

    // Invariant 4: a pair of a missing vault, then a pair of unregistered
    // assets, is refused.
    let missing = Pair::new(Address::new_from_array([42; 32]), pair.token_mint);
    assert_maker_error(
        market_maker
            .update_config(ConfigUpdate {
                add_pairs: vec![missing],
                ..ConfigUpdate::default()
            })
            .await,
        "VaultMissing of the vault that does not exist",
        |error| matches!(error, MakerError::VaultMissing { vault } if *vault == missing.vault),
    );
    let unregistered = blocking(|| -> Result<Pair> {
        let unregistered = create_vault(rpc, &fixture::payer(), pair.token_mint)?;
        wait_for_account(rpc, &unregistered.vault, VAULT_VISIBLE_TIMEOUT)?;
        Ok(unregistered)
    })?;
    assert_maker_error(
        market_maker
            .update_config(ConfigUpdate {
                add_pairs: vec![unregistered],
                ..ConfigUpdate::default()
            })
            .await,
        "AssetNotRegistered of the unregistered shares mint",
        |error| {
            matches!(
                error,
                MakerError::AssetNotRegistered { mint } if *mint == unregistered.shares_mint
            )
        },
    );

    // Invariant 5: the removed pair is re-added once retired.
    readd_after_retirement(&market_maker, &pair).await?;
    market_maker
        .quote(&pair, Direction::Withdrawal, WITHDRAW_SHARES)
        .await?;
    market_maker.shutdown().await;
    Ok(())
}

// Test fixtures and shared helpers.

/// Re-adds `pair` once it has retired. Until then the update must fail with
/// exactly [`ConfigError::PairExists`] for `pair`; any other error, or still
/// existing after [`RETIRE_TIMEOUT`], fails the test.
async fn readd_after_retirement(market_maker: &MarketMaker, pair: &Pair) -> Result<()> {
    let deadline = Instant::now() + RETIRE_TIMEOUT;
    loop {
        let readded = market_maker
            .update_config(ConfigUpdate {
                add_pairs: vec![*pair],
                ..ConfigUpdate::default()
            })
            .await;
        let Err(error) = readded else {
            return Ok(());
        };
        match error.downcast::<MakerError>() {
            Ok(MakerError::Config(ConfigError::PairExists { vault })) if vault == pair.vault => {
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "pair {vault} still exists after {RETIRE_TIMEOUT:?}, want it retired"
                    ));
                }
                tokio::time::sleep(POLL).await;
            }
            Ok(error) => return Err(anyhow!("got {error:?}, want PairExists until retired")),
            Err(error) => return Err(anyhow!("got {error:?}, want PairExists until retired")),
        }
    }
}

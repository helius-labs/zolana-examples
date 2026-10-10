//! Tested invariants:
//! 1. The maker refuses a quote whose fill would push an asset's net balance
//!    outside its target range, with `OutsideTargetRange` naming that balance.
//! 2. Of two concurrent fills that each fit the range alone but not together,
//!    exactly one is admitted; the other is refused with `OutsideTargetRange`.
//! 3. A landed swap moves the user's holdings and the maker's net balance by
//!    exactly the quoted `amount_in` and `amount_out`, without a rebalance.
//! 4. A net balance above the range triggers one automatic rebalance that
//!    brings both assets back inside their ranges.
//! 5. A kVault operation shields what it paid into the maker's share
//!    account except a margin of `max(1, 1 bps)` (exact on the uninvested
//!    localnet vault, whose simulation matches the execution), and the next
//!    one sweeps that residual up to 10 bps of its own payout.
//! 6. With both assets above their max at once, no rebalance fits both
//!    ranges: after at least `REFUSED_CHECKS` range checks found the pair out
//!    of range, the maker has triggered none and its public balances and the
//!    vault are unchanged.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use zolana_client::{SolanaRpc, ZolanaClient};

use k_lend_market_maker::{Holdings, MakerFill, MarketMaker, TargetRange, TokenConfig};
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Direction, Quote, SwapError},
};

use k_lend_rfq_test_utils::{
    assert::assert_swap_error,
    chain::{confirm_indexed, public_balances, read_vault},
    market_maker::{net_holdings, share_account, shield_margin, sweep_cap, wait_for_rebalance},
    setup::{setup_with, SetupConfig, TestEnv, REBALANCE_TIMEOUT, SEED_DEPOSIT, SETTLE_GRACE},
    user::User,
};

const TEST_NUMBER: u16 = 18;
const EXTRA_USERS: u8 = 3;
const USER_COLLATERAL: u64 = 80_000_000;
const SEED_COLLATERAL: u64 = 20_000_000;
const LARGE_COLLATERAL: u64 = 35_000_000;
const SMALL_COLLATERAL: u64 = 4_000_000;
const OUTSIDE_COLLATERAL: u64 = 8_000_000;
const COLLATERAL_RANGE_MAX: u64 = 60_000_000;
const SHARE_RANGE_MIN: u64 = 100_000_000;
const SHARE_RANGE_MAX: u64 = 400_000_000;
const CONFLICT_TEST_NUMBER: u16 = 33;
const CONFLICT_COLLATERAL_MAX: u64 = 1_000_000;
const CONFLICT_SHARE_MAX: u64 = 1_000_000;
/// Range checks that must have refused a rebalance before invariant 6 is
/// asserted: two, so at least one check ran after the first was handled.
const REFUSED_CHECKS: usize = 2;
/// Longest invariant 6 waits for `REFUSED_CHECKS` refusals.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(60);

/// Invariants 1-5: quotes and fills that would leave a range are refused,
/// swaps inside it move both holdings by the quoted amounts, and a balance
/// pushed above the range triggers a rebalance back into it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn target_ranges_bound_quotes_and_trigger_rebalances() -> Result<()> {
    let collateral_range = TargetRange::new(0, COLLATERAL_RANGE_MAX)?;
    let share_range = TargetRange::new(SHARE_RANGE_MIN, SHARE_RANGE_MAX)?;
    let TestEnv {
        localnet,
        user,
        users,
        market_maker,
        pair,
        ..
    } = setup_with(SetupConfig {
        extra_users: EXTRA_USERS,
        user_collateral: USER_COLLATERAL,
        collateral: TokenConfig {
            range: Some(collateral_range),
            profile: None,
        },
        shares: TokenConfig {
            range: Some(share_range),
            profile: None,
        },
        ..SetupConfig::new(TEST_NUMBER)
    })
    .await?;
    let rpc = localnet.client.rpc();
    let mut users = users.into_iter();
    let mut depositor = user;
    let mut second = users.next().ok_or_else(|| anyhow!("no second user"))?;
    let mut third = users.next().ok_or_else(|| anyhow!("no third user"))?;
    let mut fourth = users.next().ok_or_else(|| anyhow!("no fourth user"))?;

    // Invariant 5: the seed shields all it minted but the margin.
    let seeded = market_maker
        .seed_inventory(&pair, SEED_DEPOSIT, SEED_COLLATERAL)
        .await?;
    let share_account_after_seed = share_account(rpc, &market_maker, &pair)?;
    assert_eq!(share_account_after_seed, shield_margin(seeded.shares));
    let shares_after_seed = market_maker.net_balance(&pair.shares_mint);
    let want_shares = seeded.shares - share_account_after_seed;
    assert_eq!(
        shares_after_seed, want_shares,
        "maker shares after the seed: got {shares_after_seed}, want {want_shares}"
    );
    let collateral_after_seed = market_maker.net_balance(&pair.token_mint);
    let vault_after_seed = read_vault(rpc, &pair)?;

    // Invariant 1: a quote leaving the collateral range is refused.
    let deposit = swap(
        &localnet.client,
        &pair,
        &mut depositor,
        &market_maker,
        Direction::Deposit,
        LARGE_COLLATERAL,
    )
    .await?;
    let outside = SwapError::OutsideTargetRange {
        asset: pair.token_mint,
        balance_after: collateral_after_seed + deposit.amount_in + LARGE_COLLATERAL,
        min: collateral_range.min(),
        max: collateral_range.max(),
    };
    assert_swap_error(
        market_maker
            .quote(&pair, Direction::Deposit, LARGE_COLLATERAL)
            .await,
        &format!("{outside:?}"),
        |error| *error == outside,
    );

    // Invariant 3: swaps inside the range land without a rebalance.
    let withdrawal = swap(
        &localnet.client,
        &pair,
        &mut depositor,
        &market_maker,
        Direction::Withdrawal,
        deposit.amount_out,
    )
    .await?;
    let redeposit = swap(
        &localnet.client,
        &pair,
        &mut second,
        &market_maker,
        Direction::Deposit,
        LARGE_COLLATERAL,
    )
    .await?;
    assert_eq!(market_maker.rebalances(), Vec::new());
    assert_eq!(read_vault(rpc, &pair)?, vault_after_seed);
    let collateral = market_maker.net_balance(&pair.token_mint);
    let want =
        collateral_after_seed + deposit.amount_in + redeposit.amount_in - withdrawal.amount_out;
    assert_eq!(
        collateral, want,
        "maker collateral after three swaps: got {collateral}, want {want}"
    );
    assert!(
        collateral_range.contains(collateral),
        "maker collateral after three swaps: got {collateral}, want within {collateral_range:?}"
    );

    // Invariant 2: of two concurrent fills only one fits the range.
    let quotes = [
        market_maker
            .quote(&pair, Direction::Deposit, SMALL_COLLATERAL)
            .await?,
        market_maker
            .quote(&pair, Direction::Deposit, SMALL_COLLATERAL)
            .await?,
    ];
    let maker_before = net_holdings(&market_maker, &pair);
    let third_before = third.holdings(&pair)?;
    let fourth_before = fourth.holdings(&pair)?;
    let [first_offer, second_offer] = quotes;
    let orders = [
        third.order(&localnet.client, &pair, &first_offer).await?,
        fourth.order(&localnet.client, &pair, &second_offer).await?,
    ];
    let [first_order, second_order] = &orders;
    let (first_fill, second_fill) = tokio::join!(
        market_maker.fill(&pair, &first_order.request),
        market_maker.fill(&pair, &second_order.request),
    );
    let (fill, user, user_before, order, offer, refused) = match (first_fill, second_fill) {
        (Ok(fill), Err(refused)) => (
            fill,
            &mut third,
            third_before,
            first_order,
            first_offer,
            refused,
        ),
        (Err(refused), Ok(fill)) => (
            fill,
            &mut fourth,
            fourth_before,
            second_order,
            second_offer,
            refused,
        ),
        (first, second) => {
            return Err(anyhow!(
                "expected exactly one fill, got {:?} and {:?}",
                first.map(|fill| fill.fill.expires_at),
                second.map(|fill| fill.fill.expires_at)
            ))
        }
    };
    let outside = SwapError::OutsideTargetRange {
        asset: pair.token_mint,
        balance_after: collateral + 2 * SMALL_COLLATERAL,
        min: collateral_range.min(),
        max: collateral_range.max(),
    };
    assert_swap_error::<MakerFill>(Err(refused), &format!("{outside:?}"), |error| {
        *error == outside
    });

    // Invariant 3: the admitted fill moves both holdings by its quote.
    user.complete(&localnet.client, &market_maker, &pair, order, &fill)
        .await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: user_before.collateral - offer.quote.amount_in,
            shares: user_before.shares + offer.quote.amount_out,
        }
    );
    let maker_after = net_holdings(&market_maker, &pair);
    assert_eq!(
        maker_after,
        Holdings {
            collateral: maker_before.collateral + offer.quote.amount_in,
            shares: maker_before.shares - offer.quote.amount_out,
        }
    );
    assert!(
        collateral_range.contains(maker_after.collateral),
        "maker collateral after the admitted fill: got {}, want within {collateral_range:?}",
        maker_after.collateral
    );
    assert_eq!(market_maker.rebalances(), Vec::new());

    // Invariant 4: collateral pushed above the range triggers a rebalance.
    market_maker
        .seed_inventory(&pair, 0, OUTSIDE_COLLATERAL)
        .await?;
    let accumulated = maker_after.collateral + OUTSIDE_COLLATERAL;
    assert!(
        accumulated > collateral_range.max(),
        "accumulated collateral: got {accumulated}, want above {}",
        collateral_range.max()
    );

    let signature = wait_for_rebalance(&market_maker, REBALANCE_TIMEOUT).await?;
    confirm_indexed(&localnet.client, signature, "rebalance")?;
    tokio::time::sleep(SETTLE_GRACE).await;
    market_maker.sync().await?;
    assert_eq!(market_maker.rebalances(), vec![signature]);
    let vault_after = read_vault(rpc, &pair)?;
    let deposited = vault_after.token_available - vault_after_seed.token_available;
    let minted = vault_after.shares_issued - vault_after_seed.shares_issued;
    let holdings = net_holdings(&market_maker, &pair);
    let share_account_after = share_account(rpc, &market_maker, &pair)?;
    // Invariant 5: the rebalance sweeps the seed's residual up to its cap and
    // leaves its own margin.
    let swept = share_account_after_seed.min(sweep_cap(minted));
    assert_eq!(
        share_account_after,
        share_account_after_seed - swept + shield_margin(minted)
    );
    assert_eq!(
        holdings,
        Holdings {
            collateral: accumulated - deposited,
            shares: maker_after.shares + share_account_after_seed + minted - share_account_after,
        }
    );
    assert!(
        collateral_range.contains(holdings.collateral),
        "maker collateral after the rebalance: got {}, want within {collateral_range:?}",
        holdings.collateral
    );
    assert!(
        share_range.contains(holdings.shares),
        "maker shares after the rebalance: got {}, want within {share_range:?}",
        holdings.shares
    );
    println!(
        "automatic rebalance deposited {deposited} of {accumulated} collateral, {} left",
        holdings.collateral
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 6: incompatible ranges produce no rebalance instead of a
/// deposit/withdraw oscillation, checked after the maker has refused a
/// rebalance `REFUSED_CHECKS` times, so the range check provably ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incompatible_ranges_trigger_no_rebalance() -> Result<()> {
    let collateral_range = TargetRange::new(0, CONFLICT_COLLATERAL_MAX)?;
    let share_range = TargetRange::new(0, CONFLICT_SHARE_MAX)?;
    let config = SetupConfig {
        collateral: TokenConfig {
            range: Some(collateral_range),
            profile: None,
        },
        shares: TokenConfig {
            range: Some(share_range),
            profile: None,
        },
        ..SetupConfig::new(CONFLICT_TEST_NUMBER)
    };
    let status_interval = config.concurrency.status_interval;
    let TestEnv {
        localnet,
        market_maker,
        pair,
        ..
    } = setup_with(config).await?;
    let rpc = localnet.client.rpc();
    let seeded = market_maker
        .seed_inventory(&pair, SEED_DEPOSIT, SEED_COLLATERAL)
        .await?;
    let public_before = public_balances(rpc, &market_maker.address(), &pair)?;
    let residual = share_account(rpc, &market_maker, &pair)?;
    assert_eq!(residual, shield_margin(seeded.shares));
    let holdings = Holdings {
        collateral: SEED_COLLATERAL,
        shares: seeded.shares - residual,
    };
    assert_eq!(market_maker.holdings(&pair), holdings);
    assert!(
        holdings.collateral > collateral_range.max(),
        "seeded collateral: got {}, want above {}",
        holdings.collateral,
        collateral_range.max()
    );
    assert!(
        holdings.shares > share_range.max(),
        "seeded shares: got {}, want above {}",
        holdings.shares,
        share_range.max()
    );
    let vault_before = read_vault(rpc, &pair)?;

    let deadline = Instant::now() + REFUSAL_TIMEOUT;
    while market_maker.range_refusals() < REFUSED_CHECKS {
        anyhow::ensure!(
            Instant::now() < deadline,
            "{} range refusals after {REFUSAL_TIMEOUT:?}, want at least {REFUSED_CHECKS}",
            market_maker.range_refusals()
        );
        tokio::time::sleep(status_interval).await;
    }

    assert_eq!(market_maker.triggered_rebalances(), 0);
    assert_eq!(market_maker.rebalances(), Vec::new());
    assert_eq!(
        public_balances(rpc, &market_maker.address(), &pair)?,
        public_before
    );
    assert_eq!(read_vault(rpc, &pair)?, vault_before);
    assert_eq!(market_maker.holdings(&pair), holdings);
    market_maker.shutdown().await;
    Ok(())
}

// Test fixtures and shared helpers.

/// Quotes `amount_in` in `direction` and fills it for `user` end to end;
/// returns the filled quote.
async fn swap(
    client: &ZolanaClient<SolanaRpc>,
    pair: &Pair,
    user: &mut User,
    market_maker: &MarketMaker,
    direction: Direction,
    amount_in: u64,
) -> Result<Quote> {
    let offer = market_maker.quote(pair, direction, amount_in).await?;
    let order = user.order(client, pair, &offer).await?;
    let fill = market_maker.fill(pair, &order.request).await?;
    user.complete(client, market_maker, pair, &order, &fill)
        .await?;
    Ok(offer.quote)
}

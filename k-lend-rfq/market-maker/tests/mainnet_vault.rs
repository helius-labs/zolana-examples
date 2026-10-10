//! Tests against a mainnet kVault written into the localnet from an account
//! snapshot (`mainnet::snapshot_vault`). They need mainnet access through
//! `KAMINO_MAINNET_RPC_URL` and skip when it is unset.
//!
//! Tested invariants:
//! 1. On an invested vault, `VaultState::deposit` previews the shares the
//!    program mints for the same deposit, never fewer and at most
//!    `1 + preview / DRIFT_DIVISOR` more, and the deposit fits the compute budget `vault_compute_units`
//!    requests for its reserve count. Not exact: the preview prices the
//!    reserves as of their last refresh on mainnet, while the program first
//!    refreshes them at the localnet clock, accruing the interest since then
//!    (about 2e-7 of the AUM in the runs on 2026-10-10, which turns the
//!    preview of a 1 USDC deposit 1 share high in some runs). The vault
//!    charges no fees, so the drift is interest only. The bound is one
//!    share of rounding plus 1e-5 of the preview, 50 times the observed
//!    drift: at a lending APY near 10% that is about an hour of interest,
//!    far longer than a mainnet reserve goes without a refresh.
//! 2. A user deposit swap and the rebalance deposit it triggers land on the
//!    invested vault (its reserves refreshed through the program's own CPI);
//!    the maker's shielded shares grow by exactly what the vault minted into
//!    its share account minus what stays there, which is the `max(1, 1 bps)`
//!    margin of the simulated payout less at most the interest drift to
//!    execution, and its collateral drops by exactly what the vault took.

use anyhow::{anyhow, Result};
use zolana_interface::pda;

use k_lend_market_maker::{
    vault_compute_units, Holdings, TargetRange, TokenConfig, MAX_COMPUTE_UNITS,
};
use k_lend_rfq_sdk::{
    kvault::{deposit_instruction, UserAccounts},
    swap::Direction,
};

use k_lend_rfq_test_utils::{
    chain::{blocking, confirm_indexed, public_balances, read_vault, simulate_token_balance},
    mainnet::{mainnet_rpc_url, snapshot_vault, MAINNET_RPC_URL_VAR, MAINNET_USDC_VAULT},
    market_maker::{net_holdings, shield_margin, sweep_cap, wait_for_rebalance},
    setup::{setup_with, SetupConfig, TestEnv, VaultSource, REBALANCE_TIMEOUT, SETTLE_GRACE},
};

const PRICE_TEST_NUMBER: u16 = 34;
const REBALANCE_TEST_NUMBER: u16 = 35;
/// 1 USDC.
const PREVIEW_DEPOSIT: u64 = 1_000_000;
/// The preview may exceed the minted shares by one share of rounding plus
/// `preview / DRIFT_DIVISOR` (1e-5) of interest drift; see invariant 1.
const DRIFT_DIVISOR: u64 = 100_000;
const SEED_DEPOSIT: u64 = 100_000_000;
const SEED_COLLATERAL: u64 = 20_000_000;
const SWAP_COLLATERAL: u64 = 5_000_000;
const OUTSIDE_COLLATERAL: u64 = 8_000_000;
const COLLATERAL_RANGE_MAX: u64 = 30_000_000;

/// Invariant 1: the SDK preview of a 1 USDC deposit matches the shares the
/// program mints up to one share plus the interest drift, within the
/// requested budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn price_matches_program_on_an_invested_vault() -> Result<()> {
    let Some(config) = snapshot_config(PRICE_TEST_NUMBER)? else {
        return Ok(());
    };
    let TestEnv {
        localnet,
        market_maker,
        market_maker_wallet,
        pair,
        ..
    } = setup_with(config).await?;
    let rpc = localnet.client.rpc();
    let state = read_vault(rpc, &pair)?;
    let reserves = state.reserves.as_slice().len();
    assert!(
        reserves > 0,
        "reserves of vault {}: got {reserves}, want at least one",
        pair.vault
    );
    let preview = state.deposit(PREVIEW_DEPOSIT)?;

    let maker = market_maker_wallet.address();
    let accounts = UserAccounts {
        user: maker,
        token_account: pda::associated_token_address(&maker, &pair.token_mint),
        shares_account: pda::associated_token_address(&maker, &pair.shares_mint),
    };
    let deposit = deposit_instruction(&pair.vault, &state, &accounts, PREVIEW_DEPOSIT)?;
    let simulated = blocking(|| {
        simulate_token_balance(
            rpc,
            &[deposit],
            market_maker_wallet.signer(),
            MAX_COMPUTE_UNITS,
            &accounts.shares_account,
        )
    })?;
    let budget = vault_compute_units(reserves);
    println!(
        "deposit of {PREVIEW_DEPOSIT} into {} reserves: preview {} shares for {} tokens, \
         simulated {} shares, {} compute units (budget {budget})",
        reserves, preview.shares, preview.tokens, simulated.token_amount, simulated.units_consumed
    );
    simulated
        .logs
        .iter()
        .filter(|line| line.contains("consumed") || line.contains("aum"))
        .for_each(|line| println!("  {line}"));
    let drift = preview.shares.checked_sub(simulated.token_amount);
    let max_drift = 1 + preview.shares / DRIFT_DIVISOR;
    assert!(
        drift.is_some_and(|drift| drift <= max_drift),
        "minted shares: got {}, want at most {max_drift} below the preview {}",
        simulated.token_amount,
        preview.shares
    );
    assert!(
        simulated.units_consumed < u64::from(budget),
        "deposit compute units: got {}, want below the budget {budget}",
        simulated.units_consumed
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 2: a user deposit swap and the rebalance deposit it triggers
/// land on the invested vault and the maker shields what the vault minted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_and_rebalance_land_on_an_invested_vault() -> Result<()> {
    let Some(config) = snapshot_config(REBALANCE_TEST_NUMBER)? else {
        return Ok(());
    };
    let collateral_range = TargetRange::new(0, COLLATERAL_RANGE_MAX)?;
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        pair,
        ..
    } = setup_with(SetupConfig {
        collateral: TokenConfig {
            range: Some(collateral_range),
            profile: None,
        },
        ..config
    })
    .await?;
    let rpc = localnet.client.rpc();
    market_maker
        .seed_inventory(&pair, SEED_DEPOSIT, SEED_COLLATERAL)
        .await?;
    let user_before = user.holdings(&pair)?;
    let maker_before = net_holdings(&market_maker, &pair);

    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    user.complete(&localnet.client, &market_maker, &pair, &order, &fill)
        .await?;
    let quote = offer.quote;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: user_before.collateral - quote.amount_in,
            shares: user_before.shares + quote.amount_out,
        }
    );
    let maker_after_swap = net_holdings(&market_maker, &pair);
    assert_eq!(
        maker_after_swap,
        Holdings {
            collateral: maker_before.collateral + quote.amount_in,
            shares: maker_before.shares - quote.amount_out,
        }
    );
    assert_eq!(market_maker.rebalances(), Vec::new());

    market_maker
        .seed_inventory(&pair, 0, OUTSIDE_COLLATERAL)
        .await?;
    let [collateral_account_before, share_account_before] =
        public_balances(rpc, &market_maker.address(), &pair)?;
    let vault_before = read_vault(rpc, &pair)?;
    let accumulated = maker_after_swap.collateral + OUTSIDE_COLLATERAL;
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
    let deposited = grown(
        vault_before.token_available,
        vault_after.token_available,
        "vault token_available",
    )? + vault_before.crank_funds;
    let minted = grown(
        vault_before.shares_issued,
        vault_after.shares_issued,
        "vault shares_issued",
    )?;
    let [collateral_account_after, share_account_after] =
        public_balances(rpc, &market_maker.address(), &pair)?;
    // The maker unshields the previewed deposit; the program takes
    // `ceil(aum * shares / shares_issued)` at execution, after the reserves
    // accrued a few more slots of interest, which can be less. The rest
    // stays in the maker's public collateral account.
    let unspent = grown(
        collateral_account_before,
        collateral_account_after,
        "maker public collateral account",
    )?;
    // The residual is the margin of the simulated payout; the program mints
    // at execution, which can be less by the same interest drift, never more.
    let swept = share_account_before.min(sweep_cap(minted));
    let residual = grown(
        swept,
        share_account_before,
        "share account residual after the sweep",
    )? + shield_margin(minted);
    assert!(
        share_account_after <= residual && residual - share_account_after <= shield_margin(minted),
        "share account: got {share_account_after}, want at most {residual} and at least {residual} - {}",
        shield_margin(minted)
    );
    let holdings = net_holdings(&market_maker, &pair);
    assert_eq!(
        holdings,
        Holdings {
            collateral: accumulated - deposited - unspent,
            shares: maker_after_swap.shares + share_account_before + minted - share_account_after,
        }
    );
    assert!(
        collateral_range.contains(holdings.collateral),
        "maker collateral after the rebalance: got {}, want within {collateral_range:?}",
        holdings.collateral
    );
    println!(
        "rebalance {signature} deposited {deposited} for {minted} shares on {} reserves",
        vault_after.reserves.as_slice().len()
    );
    market_maker.shutdown().await;
    Ok(())
}

// Test fixtures and shared helpers.

/// The setup for test `test` on a snapshot of `MAINNET_USDC_VAULT`, or
/// `None` (after one line on stdout) when `KAMINO_MAINNET_RPC_URL` is unset.
fn snapshot_config(test: u16) -> Result<Option<SetupConfig>> {
    let Some(rpc_url) = mainnet_rpc_url() else {
        println!("{MAINNET_RPC_URL_VAR} is not set; skipping the mainnet vault test");
        return Ok(None);
    };
    let accounts = blocking(|| snapshot_vault(&rpc_url, &MAINNET_USDC_VAULT))?;
    Ok(Some(SetupConfig {
        vault: VaultSource::Snapshot {
            vault: MAINNET_USDC_VAULT,
            accounts,
        },
        ..SetupConfig::new(test)
    }))
}

/// `after - before` of the balance `what`, or an error naming it when the
/// balance shrank, instead of an underflow panic.
fn grown(before: u64, after: u64, what: &str) -> Result<u64> {
    after
        .checked_sub(before)
        .ok_or_else(|| anyhow!("{what} shrank from {before} to {after}"))
}

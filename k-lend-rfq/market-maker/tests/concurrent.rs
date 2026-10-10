//! Tested invariants:
//! 1. Fills are not serialised: with `UTXOS` share UTXOs, `UTXOS` fills are
//!    proven and wait for their user's signature at the same time.
//! 2. Every concurrent swap lands; each user holds the quoted shares and the
//!    rest of its collateral, and each fill leaves one change output.
//! 3. The market maker's holdings move by exactly the swapped amounts.
//! 4. Consolidation merges the grown share UTXO set back to `UTXOS` without
//!    changing the holdings.
//! 5. A rebalance deposit of the received collateral spends every collateral
//!    UTXO and mints the shares `VaultState::deposit` predicts.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use anyhow::Result;
use solana_signature::Signature;
use tokio::{sync::Barrier, task::JoinSet};
use zolana_program_test::localnet::FixtureLocalnet;

use k_lend_market_maker::{
    ConcurrencyConfig, ConsolidateReceipt, Holdings, InventoryProfile, MarketMaker, VaultOperation,
};
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Direction, Quote},
};

use k_lend_rfq_test_utils::{
    chain::{blocking, compute_units, confirm_indexed, read_vault},
    market_maker::share_account,
    setup::{setup_with, SetupConfig, TestEnv, MIN_UTXO_VALUE},
    sync::wait_or_timeout,
    user::User,
};

const TEST_NUMBER: u16 = 16;
const UTXOS: usize = 4;
const EXTRA_USERS: u8 = 7;
const USERS: usize = 8;
const SEED_DEPOSIT_COLLATERAL: u64 = 400_000_000;
const DEPOSIT_COLLATERAL: u64 = 10_000_000;
const USER_COLLATERAL: u64 = 40_000_000;

/// Invariants 1-5: eight users swap at once against four share UTXOs; four
/// fills are open together, all swaps land, and consolidation and rebalance
/// restore the inventory.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn concurrent_swaps_land_and_inventory_is_restored() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        users,
        market_maker,
        pair,
        ..
    } = setup_with(SetupConfig {
        extra_users: EXTRA_USERS,
        concurrency: ConcurrencyConfig {
            profile: InventoryProfile::equal(UTXOS, MIN_UTXO_VALUE),
            ..ConcurrencyConfig::default()
        },
        user_collateral: USER_COLLATERAL,
        ..SetupConfig::new(TEST_NUMBER)
    })
    .await?;
    let seeded = market_maker
        .seed_inventory(&pair, SEED_DEPOSIT_COLLATERAL, 0)
        .await?;
    // The seed's tail leaves a margin of the minted shares in the public share
    // account.
    let seed_residual = share_account(localnet.client.rpc(), &market_maker, &pair)?;
    let shielded = seeded.shares - seed_residual;
    assert_eq!(market_maker.utxos(&pair.shares_mint).len(), UTXOS);

    // Invariant 1: `UTXOS` fills are open at the same time.
    let localnet = Arc::new(localnet);
    let gate = Arc::new(Gate {
        tickets: AtomicUsize::new(0),
        filled: Barrier::new(UTXOS + 1),
        checked: Barrier::new(UTXOS + 1),
    });
    let ordered = Arc::new(Barrier::new(USERS));
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for user in std::iter::once(user).chain(users) {
        tasks.spawn(deposit(
            localnet.clone(),
            pair,
            market_maker.clone(),
            user,
            ordered.clone(),
            gate.clone(),
        ));
    }
    wait_or_timeout(&gate.filled, "the first fills").await?;
    assert_eq!(market_maker.open_fills().await?, UTXOS);
    wait_or_timeout(&gate.checked, "the open fill count").await?;
    let mut settled = Vec::with_capacity(USERS);
    while let Some(joined) = tasks.join_next().await {
        settled.push(joined??);
    }
    let elapsed = started.elapsed();

    // Invariant 2: every swap lands with one change output each.
    assert_eq!(settled.len(), USERS);

    let change_outputs: Vec<usize> = settled.iter().map(|fill| fill.change_outputs).collect();
    assert_eq!(
        change_outputs,
        vec![1; USERS],
        "change outputs per fill: got {change_outputs:?}, want one each"
    );
    println!(
        "{USERS} concurrent deposits on {UTXOS} utxos in {elapsed:?}: \
         {UTXOS} open at once, one change output each"
    );

    for fill in &settled {
        let signature = fill.signature;
        confirm_indexed(&localnet.client, signature, "swap")?;
    }
    let mut shares_paid = 0;
    for fill in &mut settled {
        fill.user.sync(&localnet.client).await?;
        assert_eq!(
            fill.user.holdings(&pair)?,
            Holdings {
                collateral: USER_COLLATERAL - DEPOSIT_COLLATERAL,
                shares: fill.quote.amount_out,
            }
        );
        shares_paid += fill.quote.amount_out;
    }

    // Invariant 3: the market maker's holdings move by the swapped amounts.
    market_maker.sync().await?;
    let collateral_received = DEPOSIT_COLLATERAL * u64::try_from(USERS)?;
    assert_eq!(
        market_maker.holdings(&pair),
        Holdings {
            collateral: collateral_received,
            shares: shielded - shares_paid,
        }
    );
    // Each fill spent one share UTXO and left one change output.
    assert_eq!(market_maker.utxos(&pair.shares_mint).len(), UTXOS);
    assert_eq!(market_maker.utxos(&pair.token_mint).len(), USERS);

    // Invariant 4: consolidation restores `UTXOS` share UTXOs.
    let grown = market_maker.utxos(&pair.shares_mint).len();
    let consolidation = market_maker.consolidate(pair.shares_mint).await?;
    assert_eq!(
        consolidation,
        ConsolidateReceipt {
            signature: consolidation.signature,
            inputs: grown,
            outputs: UTXOS,
        }
    );
    market_maker.sync().await?;
    assert_eq!(market_maker.utxos(&pair.shares_mint).len(), UTXOS);
    assert_eq!(
        market_maker.holdings(&pair),
        Holdings {
            collateral: collateral_received,
            shares: shielded - shares_paid,
        }
    );

    // Invariant 5: the rebalance deposit spends every collateral UTXO.
    let rpc = localnet.client.rpc();
    let before_rebalance = read_vault(rpc, &pair)?;
    let predicted = before_rebalance.deposit(collateral_received)?;
    let rebalance = market_maker
        .rebalance_shares(&pair, collateral_received)
        .await?;
    assert_eq!(
        rebalance,
        VaultOperation {
            before: before_rebalance,
            after: predicted.after,
            tokens: collateral_received,
            shares: predicted.shares,
            inputs: USERS,
            signature: rebalance.signature,
        }
    );
    assert_eq!(
        market_maker.holdings(&pair),
        Holdings {
            collateral: 0,
            // The rebalance sweeps the seed's residual and leaves its own
            // margin unshielded.
            shares: shielded - shares_paid + rebalance.shares + seed_residual
                - share_account(rpc, &market_maker, &pair)?,
        }
    );
    assert_eq!(market_maker.utxos(&pair.token_mint), Vec::new());
    println!(
        "rebalance deposit of {} inputs: {} CU",
        rebalance.inputs,
        blocking(|| compute_units(rpc, &rebalance.signature))?
    );
    market_maker.shutdown().await;
    Ok(())
}

// Test fixtures and shared helpers.

/// One user's landed deposit swap.
struct Settled {
    user: User,
    quote: Quote,
    signature: Signature,
    change_outputs: usize,
}

/// Holds the first `UTXOS` fills between proving and settling until the test
/// has counted the market maker's open fills.
struct Gate {
    tickets: AtomicUsize,
    filled: Barrier,
    checked: Barrier,
}

/// One user's deposit of `DEPOSIT_COLLATERAL`: quotes and orders, fills once
/// every user has ordered, holds at `gate` if among the first `UTXOS` fills,
/// then verifies, signs and settles.
async fn deposit(
    localnet: Arc<FixtureLocalnet>,
    pair: Pair,
    market_maker: MarketMaker,
    user: User,
    ordered: Arc<Barrier>,
    gate: Arc<Gate>,
) -> Result<Settled> {
    let offer = market_maker
        .quote(&pair, Direction::Deposit, DEPOSIT_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    wait_or_timeout(&ordered, "all orders").await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    if gate.tickets.fetch_add(1, Ordering::SeqCst) < UTXOS {
        wait_or_timeout(&gate.filled, "the first fills").await?;
        wait_or_timeout(&gate.checked, "the open fill count").await?;
    }
    let signature = user.settle(&market_maker, &pair, &order, &fill).await?;
    Ok(Settled {
        user,
        quote: offer.quote,
        signature,
        change_outputs: fill.change.len(),
    })
}

//! Tested invariants:
//! 1. Upkeep splits the seeded shares into the configured profile: the three
//!    largest UTXOs are `shares / 4`, `shares / 8` and `shares / 16`.
//! 2. A large swap running alongside small ones is filled with
//!    `LARGE_SWAP_INPUTS` market maker inputs while each small swap spends one,
//!    and no UTXO is spent by two swaps.
//! 3. Every swap lands; each user holds the quoted shares and the rest of its
//!    collateral, and the market maker's holdings move by exactly the swapped
//!    amounts.

use std::{
    cmp::Reverse,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use solana_message::v1;
use solana_signature::Signature;
use tokio::{sync::Barrier, task::JoinSet};
use zolana_client::transaction_size;
use zolana_program_test::localnet::FixtureLocalnet;

use k_lend_market_maker::{
    ConcurrencyConfig, Holdings, InventoryProfile, MarketMaker, TokenConfig, SWAP_COMPUTE_BUDGET,
};
use k_lend_rfq_sdk::{
    message::instructions,
    pair::Pair,
    swap::{Direction, Quote},
};

use k_lend_rfq_test_utils::{
    chain::{blocking, compute_units, confirm_indexed},
    market_maker::share_account,
    setup::{setup_with, SetupConfig, TestEnv, POLL, SEED_DEPOSIT_COLLATERAL},
    sync::wait_or_timeout,
    user::User,
};

const TEST_NUMBER: u16 = 17;
const EXTRA_USERS: u8 = 4;
const USERS: usize = 5;
const USER_COLLATERAL: u64 = 80_000_000;
const LARGE_COLLATERAL: u64 = 70_000_000;
const SMALL_COLLATERAL: u64 = 2_000_000;
const LARGE_UTXOS: [u64; 3] = [4, 8, 16];
const SMALL_UTXOS: usize = 47;
const UTXOS: usize = 50;
const LARGE_SWAP_INPUTS: usize = 2;
const BUILD_TIMEOUT: Duration = Duration::from_secs(240);

/// Invariants 1-3: one large and four small deposits fill concurrently from
/// disjoint UTXOs of the upkept profile, the large one with two market maker
/// inputs, and all of them land.
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn large_swap_fills_from_large_utxos_while_small_swaps_run() -> Result<()> {
    let concurrency = ConcurrencyConfig::default();
    let min_utxo_value = concurrency.profile.min_utxo_value;
    let TestEnv {
        localnet,
        user,
        users,
        market_maker,
        pair,
        ..
    } = setup_with(SetupConfig {
        extra_users: EXTRA_USERS,
        shares: TokenConfig {
            range: None,
            profile: Some(InventoryProfile {
                large: LARGE_UTXOS.to_vec(),
                small: SMALL_UTXOS,
                min_utxo_value,
            }),
        },
        concurrency: ConcurrencyConfig {
            utxo_upkeep_delay: Some(Duration::from_millis(1)),
            ..concurrency
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
    let shielded = seeded.shares - share_account(localnet.client.rpc(), &market_maker, &pair)?;
    // Invariant 1: upkeep builds the configured profile.
    let built = Instant::now();
    wait_for_utxos(&market_maker, &pair, UTXOS).await?;
    let mut utxos = market_maker.utxos(&pair.shares_mint);
    utxos.sort_by_key(|utxo| Reverse(utxo.amount));
    let largest: Vec<u64> = utxos.iter().take(3).map(|utxo| utxo.amount).collect();
    assert_eq!(
        largest,
        LARGE_UTXOS
            .iter()
            .map(|divisor| shielded / divisor)
            .collect::<Vec<_>>()
    );
    println!(
        "{UTXOS} share utxos built in {:?}, largest {largest:?}",
        built.elapsed()
    );

    // Invariant 2: the large swap takes two inputs, the small ones one each,
    // and no UTXO is spent twice.
    let localnet = Arc::new(localnet);
    let filled = Arc::new(Barrier::new(USERS));
    let mut tasks = JoinSet::new();
    let amounts = std::iter::once(LARGE_COLLATERAL).chain(std::iter::repeat(SMALL_COLLATERAL));
    for (user, amount_in) in std::iter::once(user).chain(users).zip(amounts) {
        tasks.spawn(swap(
            localnet.clone(),
            pair,
            market_maker.clone(),
            user,
            amount_in,
            filled.clone(),
        ));
    }
    let mut swapped = Vec::with_capacity(USERS);
    while let Some(joined) = tasks.join_next().await {
        swapped.push(joined??);
    }
    swapped.sort_by_key(|swap| Reverse(swap.quote.amount_in));

    let mut spent: Vec<[u8; 32]> = swapped
        .iter()
        .flat_map(|swap| swap.spent.iter().copied())
        .collect();
    let spent_count = spent.len();
    spent.sort_unstable();
    spent.dedup();
    assert_eq!(
        spent.len(),
        spent_count,
        "distinct spent utxos: got {}, want {spent_count}",
        spent.len()
    );
    assert_eq!(
        swapped.iter().map(|swap| swap.inputs).collect::<Vec<_>>(),
        std::iter::once(LARGE_SWAP_INPUTS)
            .chain(std::iter::repeat_n(1, USERS - 1))
            .collect::<Vec<_>>()
    );
    let large = swapped.first().ok_or_else(|| anyhow!("no large swap"))?;
    let rpc = localnet.client.rpc();
    let large_signature = large.signature;
    println!(
        "large swap of {} USDC: {} market maker inputs, {} of {} bytes, {} of {} addresses, user cap {} inputs, {} CU",
        large.quote.amount_in,
        large.inputs,
        large.bytes,
        v1::MAX_TRANSACTION_SIZE,
        large.addresses,
        v1::MAX_ADDRESSES,
        large.max_user_inputs,
        blocking(|| compute_units(rpc, &large_signature))?
    );

    // Invariant 3: every swap lands and both sides move by the quotes.
    for swap in &swapped {
        let signature = swap.signature;
        confirm_indexed(&localnet.client, signature, "swap")?;
    }
    for swap in &mut swapped {
        swap.user.sync(&localnet.client).await?;
        assert_eq!(
            swap.user.holdings(&pair)?,
            Holdings {
                collateral: USER_COLLATERAL - swap.quote.amount_in,
                shares: swap.quote.amount_out,
            }
        );
    }
    market_maker.sync().await?;
    let collateral_received: u64 = swapped.iter().map(|swap| swap.quote.amount_in).sum();
    let shares_paid: u64 = swapped.iter().map(|swap| swap.quote.amount_out).sum();
    assert_eq!(
        market_maker.holdings(&pair),
        Holdings {
            collateral: collateral_received,
            shares: shielded - shares_paid,
        }
    );
    market_maker.shutdown().await;
    Ok(())
}

// Test fixtures and shared helpers.

/// One user's landed deposit swap and the shape of its transaction.
struct Swapped {
    user: User,
    quote: Quote,
    inputs: usize,
    spent: Vec<[u8; 32]>,
    bytes: usize,
    addresses: usize,
    max_user_inputs: usize,
    signature: Signature,
}

/// Syncs the market maker until it holds `utxos` unreserved share UTXOs, or
/// fails after `BUILD_TIMEOUT`.
async fn wait_for_utxos(market_maker: &MarketMaker, pair: &Pair, utxos: usize) -> Result<()> {
    let deadline = Instant::now() + BUILD_TIMEOUT;
    loop {
        market_maker.sync().await?;
        let current = market_maker.utxos(&pair.shares_mint);
        if current.len() == utxos && current.iter().all(|utxo| !utxo.reserved) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "{} of {utxos} share utxos after {BUILD_TIMEOUT:?}",
                current.len()
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// One user's deposit of `amount_in`: quotes, orders and fills, waits until
/// every user's fill is proven, then verifies, signs and settles.
async fn swap(
    localnet: Arc<FixtureLocalnet>,
    pair: Pair,
    market_maker: MarketMaker,
    user: User,
    amount_in: u64,
    filled: Arc<Barrier>,
) -> Result<Swapped> {
    let offer = market_maker
        .quote(&pair, Direction::Deposit, amount_in)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    wait_or_timeout(&filled, "all fills").await?;
    let signature = user.settle(&market_maker, &pair, &order, &fill).await?;
    let size = transaction_size(
        &market_maker.address(),
        &instructions(&fill.fill.message)?,
        SWAP_COMPUTE_BUDGET,
    )?;
    let spent = fill.spent;
    Ok(Swapped {
        user,
        quote: offer.quote,
        inputs: spent.len(),
        spent,
        bytes: size.bytes,
        addresses: size.addresses,
        max_user_inputs: offer.max_user_inputs,
        signature,
    })
}

//! Tested invariants:
//! 1. A fill naming an order id the market maker never issued is rejected with
//!    `UnknownOrder`.
//! 2. A fill after the order's TTL is rejected with `OrderExpired`, and a
//!    later `order_ttl` update does not extend an order already quoted. An
//!    expired order leaves the `open_orders` count.
//! 3. A user transfer paying the market maker less than the order's `amount_in`
//!    is rejected with `Underpaid`, and the failed fill consumed the order: the
//!    same request again is rejected with `UnknownOrder`.
//! 4. An order is consumed by its first fill; a second fill of the same order
//!    is rejected with `UnknownOrder`.
//! 5. A fill is judged against the `max_user_inputs` stored with the order,
//!    not one recomputed from the inventory at fill time: after a
//!    consolidation widens what a new quote offers, a transfer one shape
//!    wider than the stored width is rejected with `UserTransferTooWide`
//!    naming the stored width, and a transfer at the stored width is filled.
//! 6. The market maker refuses to quote an amount its inventory cannot pay,
//!    with `InsufficientInventory`.
//! 7. The user refuses a swap message paying less than the quoted
//!    `amount_out`, with `BelowQuote`.
//! 8. The user refuses a swap message whose market maker transfer pays the user
//!    nothing, with `UnexpectedOutputs { received: 0 }`.
//! 9. The user refuses to build an order needing more inputs than the offer
//!    allows, with `TooManyInputs`.
//! 10. The fill's co-sign deadline never exceeds the order's expiry
//!     (`quoted_at + order_ttl`), and the market maker refuses a settle after
//!     that deadline and the release grace, with `UnknownFill`.
//! 11. The market maker refuses a settle carrying a signature that does not
//!     verify, with `InvalidUserSignature`, and the fill stays open for the
//!     real signature, which lands the swap.
//! 12. The user refuses a swap message of any other shape than its transfer
//!     and the market maker's transfer carrying the order address, paid by
//!     the offer's fee payer: a third (system transfer) instruction and
//!     another fee payer with `UnexpectedTransaction`, a market maker
//!     transfer naming the user's signer with `UnexpectedSigner`, and a user
//!     transfer altered in one byte with `UserTransferAltered`.
//! 13. With the fee raised to `FULL_BPS`, a quote pays nothing and is refused
//!     with `QuoteZero` before an order opens.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_signature::Signature;
use zolana_client::{compile_message, Rpc};
use zolana_keypair::ShieldedAddress;

use k_lend_market_maker::{
    ConcurrencyConfig, ConfigUpdate, Holdings, InventoryProfile, MarketMakerError, TokenConfig,
    SWAP_COMPUTE_BUDGET,
};
use k_lend_rfq_sdk::{
    swap::{Direction, Offer, OrderId, SwapError, SwapRequest, FULL_BPS},
    transfer::{smallest_shape, USER_OUTPUTS},
};

use k_lend_rfq_test_utils::{
    assert::{assert_market_maker_error, assert_swap_error},
    chain::{blocking, compile_swap, confirm_indexed, read_vault},
    market_maker::{largest_utxo, plain_deposit_out},
    setup::{
        setup, setup_with, SetupConfig, TestEnv, FEE_BPS, SEED_DEPOSIT_COLLATERAL,
        USER_SHIELD_COLLATERAL,
    },
};

const SWAP_COLLATERAL: u64 = 10_000_000;
/// More than one of the user's two 50_000_000 collateral UTXOs.
const TWO_UTXO_COLLATERAL: u64 = 60_000_000;
/// Long enough for the user to prove its transfer before the order expires.
const SHORT_ORDER_TTL: Duration = Duration::from_secs(5);
const LONG_ORDER_TTL: Duration = Duration::from_secs(600);
/// Long enough that both proofs finish before the order expires on a loaded
/// CI runner, where parallel tests share the CPU with their provers.
const QUOTE_TTL: Duration = Duration::from_secs(45);
/// Shares seeded for the width test, shielded as `FRAGMENTS` equal UTXOs.
const FRAGMENTS: usize = 6;
/// Collateral of the width test's deposits: its shares need five of the
/// `FRAGMENTS` share UTXOs (four hold `4 / 6` of `SEED_DEPOSIT_COLLATERAL`,
/// less than the quote), so the market maker transfer is five inputs and the
/// order address slot wide at quote time, a 6x2 shape that leaves the user 24
/// inputs; after consolidation one input and the slot (2x2) leave it 32.
const FRAGMENTED_COLLATERAL: u64 = 145_000_000;
/// User collateral of the width test: two UTXOs, each above
/// `FRAGMENTED_COLLATERAL`.
const WIDE_USER_COLLATERAL: u64 = 300_000_000;
/// The system program's `Transfer` instruction tag (`SystemInstruction`
/// variant 2, encoded as a little-endian u32).
const SYSTEM_TRANSFER_TAG: [u8; 4] = 2u32.to_le_bytes();
/// Lamports of the extra system transfer in the shape test.
const EXTRA_TRANSFER_LAMPORTS: u64 = 1;
/// The time after a fill's deadline by which the market maker has released it.
const RELEASE_GRACE: Duration = Duration::from_secs(5);

/// Invariant 1: a fill naming an order id the market maker never issued fails
/// with `UnknownOrder`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fill_rejects_unknown_order() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig::new(22)).await?;
    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let unknown = OrderId::random();
    let request = SwapRequest {
        order: unknown,
        user: order.request.user,
        transfer: order.request.transfer.clone(),
    };

    assert_swap_error(
        market_maker.fill(&pair, &request).await,
        "UnknownOrder of the unknown id",
        |error| matches!(error, SwapError::UnknownOrder { order } if *order == unknown),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 2: a fill after the order's TTL fails with `OrderExpired`, also
/// when the TTL was raised after the quote. `open_orders` counts the order
/// after the quote and no longer after the expired fill.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fill_rejects_expired_order() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig {
        order_ttl: SHORT_ORDER_TTL,
        ..SetupConfig::new(23)
    })
    .await?;

    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let quoted_at = Instant::now();
    let open_after_quote = market_maker.open_orders();
    assert_eq!(
        open_after_quote, 1,
        "open orders after the quote: got {open_after_quote}, want 1"
    );
    let order = user.order(&localnet.client, &pair, &offer).await?;
    tokio::time::sleep_until((quoted_at + SHORT_ORDER_TTL).into()).await;
    assert_swap_error(
        market_maker.fill(&pair, &order.request).await,
        "OrderExpired of the quoted order",
        |error| matches!(error, SwapError::OrderExpired { order } if *order == offer.id),
    );
    let open_after_fill = market_maker.open_orders();
    assert_eq!(
        open_after_fill, 0,
        "open orders after the expired fill: got {open_after_fill}, want 0"
    );

    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let quoted_at = Instant::now();
    let order = user.order(&localnet.client, &pair, &offer).await?;
    market_maker
        .update_config(ConfigUpdate {
            order_ttl: Some(LONG_ORDER_TTL),
            ..ConfigUpdate::default()
        })
        .await?;
    tokio::time::sleep_until((quoted_at + SHORT_ORDER_TTL).into()).await;
    assert_swap_error(
        market_maker.fill(&pair, &order.request).await,
        "OrderExpired of the order quoted before the ttl update",
        |error| matches!(error, SwapError::OrderExpired { order } if *order == offer.id),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 3: a user transfer paying `amount_in - 1` under a real order
/// fails with `Underpaid`, and retrying it fails with `UnknownOrder`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fill_rejects_underpaid_order() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig::new(24)).await?;
    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let amount_in = offer.quote.amount_in;
    let user_wallet = user.wallet();
    let underpaying = user_wallet.transfer(
        &localnet,
        vec![user_wallet.first_utxo(pair.token_mint)?],
        amount_in - 1,
        offer.market_maker,
        offer.fee_payer,
    )?;
    let request = SwapRequest {
        order: offer.id,
        user: user.identity(),
        transfer: underpaying.instruction,
    };

    assert_swap_error(
        market_maker.fill(&pair, &request).await,
        "Underpaid { expected: amount_in, received: amount_in - 1 }",
        |error| {
            matches!(
                error,
                SwapError::Underpaid { expected, received }
                    if *expected == amount_in && *received == amount_in - 1
            )
        },
    );
    assert_swap_error(
        market_maker.fill(&pair, &request).await,
        "UnknownOrder of the order the underpaid fill consumed",
        |error| matches!(error, SwapError::UnknownOrder { order } if *order == offer.id),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 4: the first fill consumes the order; filling it again fails
/// with `UnknownOrder`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fill_rejects_already_filled_order() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig::new(25)).await?;
    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    user.verify_quote(&pair, &order, &fill.fill.message)?;

    assert_swap_error(
        market_maker.fill(&pair, &order.request).await,
        "UnknownOrder of the filled order",
        |error| matches!(error, SwapError::UnknownOrder { order } if *order == offer.id),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 5: two orders are quoted while the shares are split into
/// `FRAGMENTS` UTXOs (a five-input market maker transfer); a consolidation into
/// one UTXO then makes a new quote offer a wider user transfer. The first
/// order's fill one shape above its stored width fails with
/// `UserTransferTooWide` naming the stored width, which a fill-time
/// recomputation would have accepted; the second order's fill at the stored
/// width succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fill_rejects_transfer_wider_than_offered() -> Result<()> {
    let min_utxo_value = ConcurrencyConfig::default().profile.min_utxo_value;
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig {
        shares: TokenConfig {
            range: None,
            profile: Some(InventoryProfile::equal(FRAGMENTS, min_utxo_value)),
        },
        order_ttl: LONG_ORDER_TTL,
        user_collateral: WIDE_USER_COLLATERAL,
        ..SetupConfig::new(26)
    })
    .await?;
    let fragments = market_maker.spendable(&pair.shares_mint).len();
    assert_eq!(
        fragments, FRAGMENTS,
        "seeded share utxos: got {fragments}, want {FRAGMENTS}"
    );
    let rejected = market_maker
        .quote(&pair, Direction::Deposit, FRAGMENTED_COLLATERAL)
        .await?;
    let filled = market_maker
        .quote(&pair, Direction::Deposit, FRAGMENTED_COLLATERAL)
        .await?;
    let stored = rejected.max_user_inputs;

    market_maker
        .update_config(ConfigUpdate {
            utxo_profiles: vec![(pair.shares_mint, InventoryProfile::equal(1, min_utxo_value))],
            ..ConfigUpdate::default()
        })
        .await?;
    market_maker.consolidate(pair.shares_mint).await?;
    market_maker.sync().await?;
    let recomputed = market_maker
        .quote(&pair, Direction::Deposit, FRAGMENTED_COLLATERAL)
        .await?
        .max_user_inputs;
    let wide = smallest_shape(stored + 1, USER_OUTPUTS)
        .ok_or_else(|| anyhow!("no shape wider than the stored {stored} inputs"))?
        .n_inputs();
    assert!(
        wide <= recomputed,
        "a fill-time recomputation must accept the wide transfer: got {wide} inputs, recomputed width {recomputed}, stored {stored}"
    );

    let order = user
        .order_with_width(&localnet.client, &pair, &rejected, Some(wide))
        .await?;
    assert_swap_error(
        market_maker.fill(&pair, &order.request).await,
        "UserTransferTooWide { inputs: wide, max: stored }",
        |error| {
            matches!(
                error,
                SwapError::UserTransferTooWide { inputs, max }
                    if *inputs == wide && *max == stored
            )
        },
    );
    let order = user
        .order_with_width(&localnet.client, &pair, &filled, Some(stored))
        .await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    user.verify_quote(&pair, &order, &fill.fill.message)?;
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 6: a quote the market maker's empty share inventory cannot pay
/// fails with `InsufficientInventory` for the shares the plain share math
/// prices.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quote_rejects_unfunded_inventory() -> Result<()> {
    let TestEnv {
        localnet,
        market_maker,
        pair,
        ..
    } = setup(27).await?;
    let rpc = localnet.client.rpc();
    let vault = read_vault(rpc, &pair)?;
    let required_shares = plain_deposit_out(&vault, SWAP_COLLATERAL, FEE_BPS)?;

    assert_swap_error(
        market_maker
            .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
            .await,
        "InsufficientInventory of shares with nothing available",
        |error| {
            matches!(
                error,
                SwapError::InsufficientInventory { asset, required, available: 0 }
                    if *asset == pair.shares_mint && *required == required_shares
            )
        },
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 7: a swap message whose market maker transfer pays
/// `amount_out - 1` fails the user's check with `BelowQuote`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_rejects_fill_below_quote() -> Result<()> {
    let env = funded(SetupConfig::new(28)).await?;
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = &env;
    let offer = market_maker
        .quote(pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, pair, &offer).await?;
    let quoted = offer.quote.amount_out;
    let short = market_maker_transfer(&env, user.identity(), quoted - 1, offer.id)?;
    let message = compile_swap(
        localnet.client.rpc(),
        &offer.fee_payer,
        &[order.request.transfer.clone(), short],
    )?;

    assert_swap_error(
        user.verify_quote(pair, &order, &message),
        "BelowQuote { quoted: amount_out, offered: amount_out - 1 }",
        |error| {
            matches!(
                error,
                SwapError::BelowQuote { quoted: want, offered }
                    if *want == quoted && *offered == quoted - 1
            )
        },
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 8: a swap message whose market maker transfer pays the
/// market maker itself fails the user's check with
/// `UnexpectedOutputs { received: 0 }`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_rejects_misdirected_fill() -> Result<()> {
    let env = funded(SetupConfig::new(29)).await?;
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = &env;
    let offer = market_maker
        .quote(pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, pair, &offer).await?;
    let misdirected = market_maker_transfer(
        &env,
        market_maker.identity(),
        offer.quote.amount_out,
        offer.id,
    )?;
    let message = compile_swap(
        localnet.client.rpc(),
        &offer.fee_payer,
        &[order.request.transfer.clone(), misdirected],
    )?;

    assert_swap_error(
        user.verify_quote(pair, &order, &message),
        "UnexpectedOutputs { received: 0 }",
        |error| matches!(error, SwapError::UnexpectedOutputs { received: 0 }),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 9: the user refuses to build an order needing two inputs
/// against an offer capped at one, with `TooManyInputs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn order_rejects_more_inputs_than_offered() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig::new(30)).await?;
    let offer = market_maker
        .quote(&pair, Direction::Deposit, TWO_UTXO_COLLATERAL)
        .await?;
    let capped = Offer {
        max_user_inputs: 1,
        ..offer
    };

    assert_swap_error(
        user.order(&localnet.client, &pair, &capped).await,
        "TooManyInputs { needed: 2, max: 1 }",
        |error| matches!(error, SwapError::TooManyInputs { needed: 2, max: 1 }),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 10: the fill's deadline is at most `quoted_at + QUOTE_TTL`; a
/// settle after it and the release grace fails with `UnknownFill`, and the
/// fill's inputs are released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_rejects_late_signature() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig {
        order_ttl: QUOTE_TTL,
        ..SetupConfig::new(31)
    })
    .await?;
    let utxos_before = market_maker.utxos(&pair.shares_mint);
    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order_expiry = Instant::now() + QUOTE_TTL;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    assert!(
        fill.fill.expires_at <= order_expiry,
        "fill deadline {:?} after the order expiry {order_expiry:?}",
        fill.fill.expires_at
    );
    user.verify_quote(&pair, &order, &fill.fill.message)?;
    let user_signature = user.sign(&fill.fill.message)?;
    tokio::time::sleep_until((fill.fill.expires_at + RELEASE_GRACE).into()).await;
    let utxos_after_release = market_maker.utxos(&pair.shares_mint);
    assert_eq!(
        utxos_after_release, utxos_before,
        "share utxos after the release: got {utxos_after_release:?}, want {utxos_before:?}"
    );

    assert_market_maker_error(
        market_maker.settle(&fill.fill, user_signature).await,
        "UnknownFill",
        |error| matches!(error, MarketMakerError::UnknownFill),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 11: a settle with a signature that does not verify fails with
/// `InvalidUserSignature` and leaves the fill open for the real signature.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_rejects_bad_signature_and_keeps_fill_open() -> Result<()> {
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        pair,
        ..
    } = funded(SetupConfig::new(32)).await?;
    let client = &localnet.client;
    let offer = market_maker
        .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    user.verify_quote(&pair, &order, &fill.fill.message)?;
    let user_address = user.wallet().signer().pubkey();

    assert_market_maker_error(
        market_maker
            .settle(&fill.fill, Signature::from([7u8; 64]))
            .await,
        "InvalidUserSignature of the user's signer",
        |error| {
            matches!(
                error,
                MarketMakerError::InvalidUserSignature { signer } if *signer == user_address
            )
        },
    );

    let user_signature = user.sign(&fill.fill.message)?;
    let signature = market_maker.settle(&fill.fill, user_signature).await?;
    confirm_indexed(client, signature, "swap")?;
    user.sync(client).await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: USER_SHIELD_COLLATERAL - SWAP_COLLATERAL,
            shares: offer.quote.amount_out,
        }
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 12: four messages that differ from the expected swap in one
/// respect each fail the user's check with the variant of that check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_rejects_unexpected_message_shape() -> Result<()> {
    let env = funded(SetupConfig::new(36)).await?;
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = &env;
    let offer = market_maker
        .quote(pair, Direction::Deposit, SWAP_COLLATERAL)
        .await?;
    let order = user.order(&localnet.client, pair, &offer).await?;
    let user_transfer = order.request.transfer.clone();
    let market_maker_instruction =
        market_maker_transfer(&env, user.identity(), offer.quote.amount_out, offer.id)?;
    let (blockhash, _) = blocking(|| localnet.client.rpc().get_latest_blockhash())?;
    let compile = |payer: &Address, instructions: &[Instruction]| {
        compile_message(payer, instructions, blockhash, SWAP_COMPUTE_BUDGET)
    };

    let extra = system_transfer(&offer.fee_payer, &market_maker.address());
    let three = compile(
        &offer.fee_payer,
        &[
            user_transfer.clone(),
            market_maker_instruction.clone(),
            extra,
        ],
    )?;
    assert_swap_error(
        user.verify_quote(pair, &order, &three),
        "UnexpectedTransaction for a third instruction",
        |error| matches!(error, SwapError::UnexpectedTransaction),
    );

    let other_payer = Address::new_unique();
    let wrong_payer = compile(
        &other_payer,
        &[user_transfer.clone(), market_maker_instruction.clone()],
    )?;
    assert_swap_error(
        user.verify_quote(pair, &order, &wrong_payer),
        "UnexpectedTransaction for another fee payer",
        |error| matches!(error, SwapError::UnexpectedTransaction),
    );

    let user_signer = user.wallet().signer().pubkey();
    let mut signing_market_maker = market_maker_instruction.clone();
    signing_market_maker
        .accounts
        .push(AccountMeta::new_readonly(user_signer, true));
    let user_signs_market_maker = compile(
        &offer.fee_payer,
        &[user_transfer.clone(), signing_market_maker],
    )?;
    assert_swap_error(
        user.verify_quote(pair, &order, &user_signs_market_maker),
        "UnexpectedSigner of the user's signer",
        |error| matches!(error, SwapError::UnexpectedSigner { signer } if *signer == user_signer),
    );

    let mut altered = user_transfer;
    let last = altered
        .data
        .last_mut()
        .ok_or_else(|| anyhow!("the user transfer has no data"))?;
    *last ^= 1;
    let altered = compile(&offer.fee_payer, &[altered, market_maker_instruction])?;
    assert_swap_error(
        user.verify_quote(pair, &order, &altered),
        "UserTransferAltered",
        |error| matches!(error, SwapError::UserTransferAltered),
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 13: a deposit the vault prices to a positive share amount
/// quotes to zero after `update_config` sets `fee_bps = FULL_BPS`, and fails
/// with `QuoteZero`; no order opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quote_rejects_zero_payout() -> Result<()> {
    let TestEnv {
        localnet,
        market_maker,
        pair,
        ..
    } = setup(37).await?;
    let rpc = localnet.client.rpc();
    let vault = read_vault(rpc, &pair)?;
    let gross = plain_deposit_out(&vault, SWAP_COLLATERAL, 0)?;
    assert!(
        gross > 0,
        "the vault prices {SWAP_COLLATERAL} to {gross} shares before the fee"
    );
    market_maker
        .update_config(ConfigUpdate {
            fee_bps: Some(FULL_BPS),
            ..ConfigUpdate::default()
        })
        .await?;

    assert_swap_error(
        market_maker
            .quote(&pair, Direction::Deposit, SWAP_COLLATERAL)
            .await,
        "QuoteZero { amount_in: SWAP_COLLATERAL }",
        |error| {
            matches!(
                error,
                SwapError::QuoteZero {
                    amount_in: SWAP_COLLATERAL
                }
            )
        },
    );
    let open = market_maker.open_orders();
    assert_eq!(
        open, 0,
        "open orders after the refused quote: got {open}, want 0"
    );
    market_maker.shutdown().await;
    Ok(())
}

// Test fixtures and shared helpers.

/// Boots the localnet of `config` and seeds the market maker with the shares of
/// a `SEED_DEPOSIT_COLLATERAL` vault deposit, so it can quote deposits.
async fn funded(config: SetupConfig) -> Result<TestEnv> {
    let env = setup_with(config).await?;
    env.market_maker
        .seed_inventory(&env.pair, SEED_DEPOSIT_COLLATERAL, 0)
        .await?;
    Ok(env)
}

/// A market maker fill transfer for order `order`: `amount` shares from the
/// market maker's largest share UTXO to `recipient`, carrying the order's
/// address, proved with the market maker's wallet outside the market maker.
fn market_maker_transfer(
    env: &TestEnv,
    recipient: ShieldedAddress,
    amount: u64,
    order: OrderId,
) -> Result<Instruction> {
    let TestEnv {
        localnet,
        market_maker,
        market_maker_wallet,
        pair,
        ..
    } = env;
    let inputs = largest_utxo(market_maker, &pair.shares_mint);
    market_maker_wallet.order_transfer(localnet, inputs, amount, recipient, order)
}

/// A system program transfer of `EXTRA_TRANSFER_LAMPORTS` from `from` to
/// `to`, built by hand: `SYSTEM_TRANSFER_TAG` followed by the lamports as a
/// little-endian u64.
fn system_transfer(from: &Address, to: &Address) -> Instruction {
    Instruction {
        program_id: Address::default(),
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data: [
            SYSTEM_TRANSFER_TAG.as_slice(),
            EXTRA_TRANSFER_LAMPORTS.to_le_bytes().as_slice(),
        ]
        .concat(),
    }
}

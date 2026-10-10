//! Tested invariants:
//! 1. An order's marker account exists once its swap lands, so a second
//!    creation of the same marker, and with it a second on-chain fill of the
//!    order, fails with `AccountAlreadyInUse` (system program custom error 0).
//! 2. The user refuses a swap message whose marker names another order, with
//!    `OrderMarkerMismatch`.

use anyhow::{anyhow, Result};
use solana_instruction_error::InstructionError;
use solana_transaction_error::TransactionError;
use zolana_client::{compile_message, sign_transaction, ClientError, Rpc};

use k_lend_market_maker::SWAP_COMPUTE_BUDGET;
use k_lend_rfq_sdk::{
    message::instructions,
    swap::{order_marker_instruction, Direction, OrderId, SwapError},
};

use k_lend_rfq_test_utils::{
    assert::assert_swap_error,
    chain::{blocking, compile_swap, confirm_indexed},
    setup::{setup, TestEnv},
};

const SEED_DEPOSIT_COLLATERAL: u64 = 50_000_000;
const SEED_COLLATERAL: u64 = 20_000_000;
const DEPOSIT_COLLATERAL: u64 = 5_000_000;

/// Invariant 1: recreating the marker of a landed order fails with
/// `AccountAlreadyInUse`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn order_marker_prevents_second_fill() -> Result<()> {
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        market_maker_wallet,
        pair,
        ..
    } = setup(20).await?;
    let client = &localnet.client;
    let rpc = client.rpc();
    market_maker
        .seed_inventory(&pair, SEED_DEPOSIT_COLLATERAL, SEED_COLLATERAL)
        .await?;

    let offer = market_maker
        .quote(&pair, Direction::Deposit, DEPOSIT_COLLATERAL)
        .await?;
    let order = user.order(client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    let signature = user.settle(&market_maker, &pair, &order, &fill).await?;
    confirm_indexed(client, signature, "swap")?;
    user.sync(client).await?;

    let marker = order_marker_instruction(&offer.fee_payer, offer.id, offer.marker_lamports)?;
    let message = compile_swap(rpc, &offer.fee_payer, &[marker])?;
    let second_marker = blocking(|| {
        rpc.process_transaction(sign_transaction(message, &[market_maker_wallet.signer()])?)
    });
    let error = second_marker
        .err()
        .ok_or_else(|| anyhow!("a second marker creation for order {} landed", offer.id))?;
    let transaction_error = match &error {
        ClientError::SolanaRpcTransaction { source, .. } => source.get_transaction_error(),
        _ => None,
    };
    assert!(
        matches!(
            transaction_error,
            Some(TransactionError::InstructionError(
                0,
                InstructionError::Custom(0)
            ))
        ),
        "got {error:?}, want InstructionError(0, Custom(0)) (AccountAlreadyInUse)"
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 2: a swap message carrying the marker of a random order id
/// fails the user's check with `OrderMarkerMismatch`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_rejects_marker_for_another_order() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        pair,
        ..
    } = setup(21).await?;
    let client = &localnet.client;
    market_maker
        .seed_inventory(&pair, SEED_DEPOSIT_COLLATERAL, SEED_COLLATERAL)
        .await?;

    let offer = market_maker
        .quote(&pair, Direction::Deposit, DEPOSIT_COLLATERAL)
        .await?;
    let order = user.order(client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    user.verify_quote(&pair, &order, &fill.fill.message)?;

    let [user_transfer, market_maker_transfer, _marker] =
        <[_; 3]>::try_from(instructions(&fill.fill.message)?)
            .map_err(|found| anyhow!("expected three instructions, found {}", found.len()))?;
    let other_marker =
        order_marker_instruction(&offer.fee_payer, OrderId::random(), offer.marker_lamports)?;
    let message = compile_message(
        &offer.fee_payer,
        &[user_transfer, market_maker_transfer, other_marker],
        *fill.fill.message.recent_blockhash(),
        SWAP_COMPUTE_BUDGET,
    )?;
    assert_swap_error(
        user.verify_quote(&pair, &order, &message),
        "OrderMarkerMismatch of the quoted order",
        |error| matches!(error, SwapError::OrderMarkerMismatch { order } if *order == offer.id),
    );
    market_maker.shutdown().await;
    Ok(())
}

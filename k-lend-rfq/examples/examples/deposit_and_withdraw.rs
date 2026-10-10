//! A private kVault deposit and withdrawal through the market maker.
//!
//! The user never touches the vault. Each swap runs: quote (the market maker
//! prices `amount_in` at the vault price minus its fee and opens an order),
//! prove (the user proves a shielded transfer of `amount_in` to the
//! market maker), fill (the market maker checks that transfer against its order
//! and adds its own transfer paying `amount_out`), verify and sign (the user),
//! settle (the market maker co-signs and sends both transfers in one
//! transaction). The market maker keeps its inventory in range with its own
//! public vault deposits and withdrawals (rebalance), unlinked from any user.
//!
//! Before signing, `QuoteCheck::verify` checks that the market maker pays the
//! fee, that the message holds exactly the user's unaltered transfer and one
//! market maker transfer, that neither transfer moves public funds, that the
//! market maker transfer carries the order address (so the order is filled at
//! most once on chain), and that the market maker pays at least the quoted
//! `amount_out`.

use anyhow::Result;
use zolana_client::{SolanaRpc, ZolanaClient};

use k_lend_market_maker::Holdings;
use k_lend_market_maker::MarketMaker;
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Direction, Quote},
};

use k_lend_rfq_test_utils::{
    chain::confirm_indexed,
    setup::{setup, TestEnv, USER_SHIELD_COLLATERAL},
    user::User,
};

const MARKET_MAKER_SEED_DEPOSIT_COLLATERAL: u64 = 200_000_000;
const MARKET_MAKER_COLLATERAL: u64 = 50_000_000;
const DEPOSIT_COLLATERAL: u64 = 40_000_000;
const WITHDRAW_SHARES: u64 = 15_000_000;
/// Index of this example's localnet; it picks the ports, so the example can
/// run next to the tests.
const EXAMPLE_LOCALNET: u16 = 12;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        pair,
        ..
    } = setup(EXAMPLE_LOCALNET).await?;

    // Market maker setup: deposit USDC into kVault and keep the shares and some
    // USDC in its private balance, so it can serve both directions.
    market_maker
        .seed_inventory(
            &pair,
            MARKET_MAKER_SEED_DEPOSIT_COLLATERAL,
            MARKET_MAKER_COLLATERAL,
        )
        .await?;

    // Deposit: the user swaps private USDC for kVault shares.
    let deposit = swap(
        &localnet.client,
        &pair,
        &mut user,
        &market_maker,
        Direction::Deposit,
        DEPOSIT_COLLATERAL,
    )
    .await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: USER_SHIELD_COLLATERAL - DEPOSIT_COLLATERAL,
            shares: deposit.amount_out,
        }
    );

    // Withdrawal: the user swaps part of its shares back for USDC.
    let withdrawal = swap(
        &localnet.client,
        &pair,
        &mut user,
        &market_maker,
        Direction::Withdrawal,
        WITHDRAW_SHARES,
    )
    .await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: USER_SHIELD_COLLATERAL - DEPOSIT_COLLATERAL + withdrawal.amount_out,
            shares: deposit.amount_out - WITHDRAW_SHARES,
        }
    );

    market_maker.shutdown().await;
    Ok(())
}

async fn swap(
    client: &ZolanaClient<SolanaRpc>,
    pair: &Pair,
    user: &mut User,
    market_maker: &MarketMaker,
    direction: Direction,
    amount_in: u64,
) -> Result<Quote> {
    // 1-2. The user requests a quote; the market maker returns `amount_out`
    // at the vault rate minus its fee, and the user's input cap.
    let offer = market_maker.quote(pair, direction, amount_in).await?;
    println!(
        "{direction:?}: {amount_in} in, {} out",
        offer.quote.amount_out
    );
    println!("order {}", offer.id);

    // 3. The user proves its transfer: its UTXOs in, `amount_in` to the
    // market maker and change back to itself.
    let order = user.order(client, pair, &offer).await?;

    // 4. The market maker checks the user transfer, proves its own transfer
    // paying `amount_out` to the user, and builds the transaction.
    let fill = market_maker.fill(pair, &order.request).await?;

    // 5. The user checks the whole message (`QuoteCheck::verify`), then
    // signs.
    user.verify_quote(pair, &order, &fill.fill.message)?;
    let user_signature = user.sign(&fill.fill.message)?;

    // 6. The market maker adds its signature and sends the transaction.
    let signature = market_maker.settle(&fill.fill, user_signature).await?;

    confirm_indexed(client, signature, "swap")?;
    user.sync(client).await?;
    market_maker.sync().await?;
    Ok(offer.quote)
}

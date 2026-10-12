//! Payment ownership, cross-order input reservations and accepted offer terms.

use anyhow::Result;
use k_lend_market_maker::{ConfigUpdate, Holdings, MarketMakerError};
use k_lend_rfq_sdk::{
    pair::Pair,
    swap::{Direction, SwapError, SwapRequest},
};
use k_lend_rfq_test_utils::{
    assert::{assert_market_maker_error, assert_swap_error},
    market_maker::{largest_utxo, net_holdings},
    setup::{setup, TestEnv, SEED_DEPOSIT_COLLATERAL, USER_SHIELD_COLLATERAL},
};
use solana_address::Address;
use solana_signature::Signature;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn payment_ownership_retries_and_accepted_fees_are_guarded() -> Result<()> {
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        market_maker_wallet,
        pair,
        ..
    } = setup(44).await?;
    market_maker
        .seed_inventory(&pair, SEED_DEPOSIT_COLLATERAL, 50_000_000)
        .await?;
    let amount = 5_000_000;
    let invalid_offer = market_maker
        .quote(&pair, Direction::Deposit, amount)
        .await?;
    let maker_payment = market_maker_wallet.transfer(
        &localnet,
        largest_utxo(&market_maker, &pair.token_mint),
        amount,
        invalid_offer.market_maker,
        invalid_offer.fee_payer,
    )?;
    let inventory_before = market_maker.utxos(&pair.shares_mint);
    let invalid_request = SwapRequest {
        order: invalid_offer.id,
        user: user.identity(),
        transfer: maker_payment.instruction,
    };
    let expected_signer = Address::new_from_array(user.identity().signing_pubkey.as_ed25519()?);
    assert_swap_error(
        market_maker.fill(&pair, &invalid_request).await,
        "UserTransferSigner of the requesting user",
        |error| matches!(error, SwapError::UserTransferSigner { expected } if *expected == expected_signer),
    );
    assert_eq!(market_maker.utxos(&pair.shares_mint), inventory_before);

    let offer = market_maker
        .quote(&pair, Direction::Deposit, amount)
        .await?;
    assert_eq!(offer.pair, pair);
    let retry_offer = market_maker
        .quote(&pair, Direction::Deposit, amount)
        .await?;
    let order = user.order(&localnet.client, &pair, &offer).await?;
    let fill = market_maker.fill(&pair, &order.request).await?;
    let before_retry = net_holdings(&market_maker, &pair);
    let retry = SwapRequest {
        order: retry_offer.id,
        user: order.request.user,
        transfer: order.request.transfer.clone(),
    };
    assert_swap_error(
        market_maker.fill(&pair, &retry).await,
        "DuplicateFill of the already reserved user input",
        |error| matches!(error, SwapError::DuplicateFill { .. }),
    );
    assert_eq!(net_holdings(&market_maker, &pair), before_retry);

    let wrong_pair = Pair::new(Address::new_unique(), pair.token_mint);
    assert_swap_error(
        user.verify_quote(&wrong_pair, &order, &fill.fill.message),
        "WrongPair",
        |error| *error == SwapError::WrongPair,
    );
    market_maker
        .update_config(ConfigUpdate {
            fee_bps: Some(offer.fee_bps + 30),
            ..ConfigUpdate::default()
        })
        .await?;
    assert_market_maker_error(
        market_maker.settle(&fill.fill, Signature::default()).await,
        "InvalidUserSignature",
        |error| matches!(error, MarketMakerError::InvalidUserSignature { signer } if *signer == expected_signer),
    );
    user.complete(&localnet.client, &market_maker, &pair, &order, &fill)
        .await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: USER_SHIELD_COLLATERAL - amount,
            shares: offer.quote.amount_out
        }
    );

    let replay_offer = market_maker
        .quote(&pair, Direction::Deposit, amount)
        .await?;
    assert_eq!(replay_offer.fee_bps, offer.fee_bps + 30);
    let replay = SwapRequest {
        order: replay_offer.id,
        user: order.request.user,
        transfer: order.request.transfer.clone(),
    };
    assert_swap_error(
        market_maker.fill(&pair, &replay).await,
        "UserInputSpent of the confirmed payment",
        |error| matches!(error, SwapError::UserInputSpent { .. }),
    );
    market_maker.shutdown().await;
    Ok(())
}

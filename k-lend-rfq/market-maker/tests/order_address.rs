//! Tested invariants:
//! 1. A landed swap creates its order address, so a second market maker
//!    transfer carrying the same order's address slot, and with it a second
//!    on-chain fill of the order, fails at the transfer with
//!    `NullifierAlreadyQueued` (shielded pool custom error 7043). The indexer
//!    already refuses the address's non-inclusion proof, so an honest builder
//!    cannot even prove the second fill.
//! 2. The user refuses a swap message whose market maker transfer carries no
//!    order address, or the address of another order, with
//!    `OrderAddressMissing`.

use anyhow::{anyhow, Result};
use solana_instruction::Instruction;
use solana_instruction_error::InstructionError;
use solana_message::VersionedMessage;
use solana_transaction_error::TransactionError;
use zolana_client::{compile_message, sign_transaction, ClientError, Rpc};
use zolana_interface::error::ShieldedPoolError;
use zolana_program::instruction::Transact;

use k_lend_market_maker::SWAP_COMPUTE_BUDGET;
use k_lend_rfq_sdk::{
    address::order_address,
    message::{instructions, transact_data},
    swap::{Direction, OrderId, SwapError},
};

use k_lend_rfq_test_utils::{
    assert::assert_swap_error,
    chain::{blocking, compile_swap, confirm_indexed},
    market_maker::largest_utxo,
    setup::{setup, TestEnv},
};

const SEED_DEPOSIT_COLLATERAL: u64 = 50_000_000;
const SEED_COLLATERAL: u64 = 20_000_000;
const DEPOSIT_COLLATERAL: u64 = 5_000_000;
/// Collateral the second, duplicate market maker transfer pays the user.
const DUPLICATE_COLLATERAL: u64 = 1_000_000;

/// Invariant 1: a market maker transfer carrying the address of a landed
/// order fails with `NullifierAlreadyQueued`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn order_address_prevents_second_fill() -> Result<()> {
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
    market_maker.sync().await?;

    // A fresh collateral input, so the order address is the only nullifier
    // of the second transfer that already exists.
    let inputs = largest_utxo(&market_maker, &pair.token_mint);
    let address = order_address(&offer.fee_payer, offer.id)?;
    let refused = market_maker_wallet
        .order_transfer(
            &localnet,
            inputs.clone(),
            DUPLICATE_COLLATERAL,
            user.identity(),
            offer.id,
        )
        .err()
        .ok_or_else(|| anyhow!("the indexer proved the landed address of {}", offer.id))?;
    assert!(
        refused.to_string().contains("already used or queued"),
        "got {refused:?}, want the indexer's already used or queued refusal"
    );

    // A second fill does not need the indexer: the address is queued, not yet
    // in the nullifier tree, so its non-inclusion holds against the current
    // root. SPP checks every nullifier PDA before it verifies the proof, so
    // a transfer proved for another order and re-pointed at the landed address
    // shows the on-chain rejection a correctly proved duplicate gets.
    let other_order = OrderId::random();
    let proved = market_maker_wallet.order_transfer(
        &localnet,
        inputs,
        DUPLICATE_COLLATERAL,
        user.identity(),
        other_order,
    )?;
    let mut data = transact_data(&proved)?;
    let other_address = order_address(&offer.fee_payer, other_order)?;
    let slot = data
        .inputs
        .iter_mut()
        .find(|input| input.nullifier_hash == other_address)
        .ok_or_else(|| anyhow!("the transfer carries no address slot"))?;
    slot.nullifier_hash = address;
    let duplicate = Transact {
        payer: offer.fee_payer,
        input_trees: vec![localnet.tree],
        output_tree: localnet.tree,
        owner_signers: Vec::new(),
        interface_transfer_accounts: Vec::new(),
        data,
    }
    .instruction();
    let message = compile_swap(rpc, &offer.fee_payer, &[duplicate])?;
    let second_fill = blocking(|| {
        rpc.process_transaction(sign_transaction(message, &[market_maker_wallet.signer()])?)
    });
    let error = second_fill
        .err()
        .ok_or_else(|| anyhow!("a second fill of order {} landed", offer.id))?;
    let transaction_error = match &error {
        ClientError::SolanaRpcTransaction { source, .. } => source.get_transaction_error(),
        _ => None,
    };
    let queued = ShieldedPoolError::NullifierAlreadyQueued as u32;
    assert_eq!(
        transaction_error,
        Some(TransactionError::InstructionError(
            0,
            InstructionError::Custom(queued)
        )),
        "got {error:?}, want InstructionError(0, Custom({queued})) (NullifierAlreadyQueued)"
    );
    market_maker.shutdown().await;
    Ok(())
}

/// Invariant 2: a real fill message whose market maker transfer is replaced
/// by one without the order address, or with another order's address, fails
/// the user's check with `OrderAddressMissing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_rejects_transact_without_order_address() -> Result<()> {
    let TestEnv {
        localnet,
        user,
        market_maker,
        market_maker_wallet,
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
    let [user_transfer, _] = <[_; 2]>::try_from(instructions(&fill.fill.message)?)
        .map_err(|found| anyhow!("expected two instructions, found {}", found.len()))?;

    let inputs = || largest_utxo(&market_maker, &pair.shares_mint);
    let amount = offer.quote.amount_out;
    let without_address = market_maker_wallet
        .transfer(
            &localnet,
            inputs(),
            amount,
            user.identity(),
            offer.fee_payer,
        )?
        .instruction;
    let other_order = market_maker_wallet.order_transfer(
        &localnet,
        inputs(),
        amount,
        user.identity(),
        OrderId::random(),
    )?;
    let swap = |market_maker_transfer: Instruction| -> Result<VersionedMessage> {
        Ok(compile_message(
            &offer.fee_payer,
            &[user_transfer.clone(), market_maker_transfer],
            *fill.fill.message.recent_blockhash(),
            SWAP_COMPUTE_BUDGET,
        )?)
    };
    for (label, market_maker_transfer) in [
        ("without an order address", without_address),
        ("with another order's address", other_order),
    ] {
        assert_swap_error(
            user.verify_quote(&pair, &order, &swap(market_maker_transfer)?),
            &format!("OrderAddressMissing of the quoted order, transfer {label}"),
            |error| matches!(error, SwapError::OrderAddressMissing { order } if *order == offer.id),
        );
    }
    market_maker.shutdown().await;
    Ok(())
}

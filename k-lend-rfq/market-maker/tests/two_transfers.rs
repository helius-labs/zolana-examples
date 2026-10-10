//! Tested invariants:
//! 1. A user transfer and a market maker transfer proved separately against the
//!    same tree carry the same tree context, so one swap-budget message
//!    combines them into one transaction.
//! 2. That transaction lands with both signatures and creates the nullifier
//!    PDA of each transfer's input.
//! 3. Both sides' holdings move by exactly the two transferred amounts.

use anyhow::{anyhow, Result};
use zolana_client::{sign_transaction, Rpc};
use zolana_interface::pda;

use k_lend_market_maker::Holdings;
use k_lend_rfq_sdk::message::{instructions, transact_data};

use k_lend_rfq_test_utils::{
    chain::{blocking, compile_swap, compute_units, confirm_indexed},
    setup::{setup, TestEnv, USER_SHIELD_COLLATERAL},
};

const MARKET_MAKER_SHIELD_COLLATERAL: u64 = 50_000_000;
const USER_PAYS_COLLATERAL: u64 = 7_000_000;
const MARKET_MAKER_PAYS_COLLATERAL: u64 = 3_000_000;

/// Invariants 1-3: two 1x2 transacts, one per side, settle in one
/// transaction and move both holdings by their amounts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_transacts_against_one_tree_settle_in_one_transaction() -> Result<()> {
    let TestEnv {
        localnet,
        mut user,
        market_maker,
        market_maker_wallet,
        collateral_mint,
        pair,
        ..
    } = setup(15).await?;
    let rpc = localnet.client.rpc();
    market_maker
        .seed_inventory(&pair, 0, MARKET_MAKER_SHIELD_COLLATERAL)
        .await?;

    let market_maker_address = market_maker.address();
    let user_transfer = user.wallet().transfer(
        &localnet,
        vec![user.wallet().first_utxo(collateral_mint)?],
        USER_PAYS_COLLATERAL,
        market_maker.identity(),
        market_maker_address,
    )?;
    let market_maker_inputs = vec![market_maker
        .spendable(&collateral_mint)
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no market maker collateral utxo"))?];
    let market_maker_transfer = market_maker_wallet.transfer(
        &localnet,
        market_maker_inputs,
        MARKET_MAKER_PAYS_COLLATERAL,
        user.identity(),
        market_maker_address,
    )?;
    // Invariant 1: both transfers name the same tree context.
    let nullifiers = [
        *user_transfer
            .nullifiers
            .first()
            .ok_or_else(|| anyhow!("user transfer spends nothing"))?,
        *market_maker_transfer
            .nullifiers
            .first()
            .ok_or_else(|| anyhow!("market maker transfer spends nothing"))?,
    ];
    let message = compile_swap(
        rpc,
        &market_maker_address,
        &[user_transfer.instruction, market_maker_transfer.instruction],
    )?;
    let roots: Vec<_> = instructions(&message)?
        .iter()
        .map(|instruction| transact_data(instruction).map(|transfer| transfer.tree_contexts))
        .collect::<Result<_>>()?;
    assert_eq!(roots.len(), 2, "transfers in the swap message");
    assert_eq!(
        roots.first(),
        roots.get(1),
        "tree contexts of the user and market maker transfers"
    );

    // Invariant 2: the transaction lands and nullifies both inputs.

    let signature = blocking(|| {
        rpc.process_transaction(sign_transaction(
            message,
            &[market_maker_wallet.signer(), user.wallet().signer()],
        )?)
    })?;
    confirm_indexed(&localnet.client, signature, "two-transfer transaction")?;
    println!(
        "two 1x2 transacts in one transaction: {} CU",
        blocking(|| compute_units(rpc, &signature))?
    );

    let nullifier_pdas: Vec<_> = nullifiers
        .iter()
        .map(|nullifier| pda::nullifier_pda(&localnet.tree, nullifier).0)
        .collect();
    assert_ne!(
        nullifier_pdas.first(),
        nullifier_pdas.get(1),
        "the two transfers share a nullifier PDA"
    );
    for nullifier_pda in &nullifier_pdas {
        assert!(
            blocking(|| rpc.get_account(*nullifier_pda))?.is_some(),
            "nullifier PDA {nullifier_pda}: got no account, want one"
        );
    }

    // Invariant 3: both holdings move by the transferred amounts.

    user.sync(&localnet.client).await?;
    market_maker.sync().await?;
    assert_eq!(
        user.holdings(&pair)?,
        Holdings {
            collateral: USER_SHIELD_COLLATERAL - USER_PAYS_COLLATERAL
                + MARKET_MAKER_PAYS_COLLATERAL,
            shares: 0,
        }
    );
    assert_eq!(
        market_maker.holdings(&pair),
        Holdings {
            collateral: MARKET_MAKER_SHIELD_COLLATERAL + USER_PAYS_COLLATERAL
                - MARKET_MAKER_PAYS_COLLATERAL,
            shares: 0,
        }
    );
    market_maker.shutdown().await;
    Ok(())
}

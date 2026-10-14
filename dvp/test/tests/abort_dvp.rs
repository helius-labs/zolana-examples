mod shared;

use anyhow::{anyhow, bail, ensure, Result};
use dvp_program::error::DvpError;
use dvp_sdk::{
    index::{index_deposits, index_own_deposits, index_terms, DiscoveredDvp},
    instructions::{
        abort_dvp::{AbortDvp, AbortDvpTransactionParams, AbortMode},
        reclaim_dvp::ReclaimDvpTransactionParams,
        settle_dvp::SettleDvpTransactionParams,
    },
    state::{Deposit, Leg, SwapDvp},
};
use dvp_test::cluster_time;
use shared::{
    assert_dvp_error, setup, Actor, TestEnv, DELIVERY_AMOUNT, INDEX_TIMEOUT, PAYMENT_AMOUNT,
};
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_program_test::fixture;
use zolana_test_utils::test_validator_asserts::wait_for_nullifier_present;
use zolana_transaction::{Mint, SOL_MINT};

/// What the seller sends above the delivery amount; an abort refunds the
/// whole deposit, surplus included.
const DELIVERY_SURPLUS: u64 = 50_000_000;
/// What each funder shields into its private balance before funding.
const SHIELD_AMOUNT: u64 = 600_000_000;
/// Seconds the settle timestamp trails the cluster clock, as in the settle
/// e2e.
const TIMESTAMP_LAG: i64 = 2;

/// The seller creates a DvP, and `discoverer` finds its terms and DvP UTXO.
fn create(env: &TestEnv, discoverer: &Actor) -> Result<(SwapDvp, DiscoveredDvp)> {
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;
    let discovered = index_terms(&discoverer.shielded, &env.localnet.client, INDEX_TIMEOUT)?;
    let [discovered] = discovered.as_slice() else {
        bail!("discovered {} DvPs", discovered.len());
    };
    Ok((terms, discovered.clone()))
}

/// The seller funds delivery with `delivery_amount` and the buyer funds
/// payment exactly.
fn fund_both(env: &TestEnv, terms: &SwapDvp, delivery_amount: u64) -> Result<()> {
    let delivery_note = env.shield(&env.seller, terms.delivery_mint, SHIELD_AMOUNT)?;
    let payment_note = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    env.fund(
        &env.seller,
        terms,
        Leg::Delivery,
        delivery_amount,
        delivery_note,
    )?;
    env.fund(
        &env.buyer,
        terms,
        Leg::Payment,
        PAYMENT_AMOUNT,
        payment_note,
    )?;
    Ok(())
}

/// Builds and proves the abort of `discovered` by `signer` with the deposits
/// it knows of.
fn prove_abort(
    env: &TestEnv,
    signer: &Actor,
    mode: AbortMode,
    discovered: &DiscoveredDvp,
    delivery_deposit: Option<Deposit>,
    payment_deposit: Option<Deposit>,
) -> Result<AbortDvp> {
    let client = &env.localnet.client;
    AbortDvpTransactionParams {
        terms: discovered.dvp_utxo.terms.clone(),
        mode,
        dvp_utxo: discovered.dvp_utxo.clone(),
        dvp_utxo_leaf_index: discovered.leaf_index,
        delivery_mint: Mint::new(fixture::spl_mint(), fixture::SPL_ASSET_ID),
        payment_mint: Mint::SOL,
        delivery_deposit,
        payment_deposit,
        payer: env.payer.pubkey(),
        tree_id: env.localnet.tree_id,
    }
    .build(&signer.shielded)?
    .prove(client.indexer(), env.localnet.tree)
}

/// The amounts the abort `signature` refunds to the seller (delivery) and to
/// the buyer (payment), as their wallets sync them.
fn refunds(env: &TestEnv, terms: &SwapDvp, signature: Signature) -> Result<(u64, u64)> {
    let refund = |actor: &Actor, mint| -> Result<u64> {
        let note = env.received_note(
            actor,
            actor.address()?.confidential_view_tag()?,
            mint,
            signature,
        )?;
        Ok(note.utxo.amount)
    };
    Ok((
        refund(&env.seller, terms.delivery_mint)?,
        refund(&env.buyer, terms.payment_mint)?,
    ))
}

/// Create a DvP and fund both legs (delivery with a surplus), then let the
/// settlement authority cancel it: both deposits come back in full to the
/// refund destinations, and the DvP UTXO and both deposits are spent. The
/// buyer cannot submit the authority's proven cancel, and a settle proven
/// before the cancel fails once the DvP is closed.
#[test]
fn authority_cancels_with_both_deposits() -> Result<()> {
    let env = setup(16)?;
    let client = &env.localnet.client;
    let tree = env.localnet.tree;
    let authority = &env.settlement_authority;
    let (terms, discovered) = create(&env, authority)?;
    fund_both(&env, &terms, DELIVERY_AMOUNT + DELIVERY_SURPLUS)?;

    let deposits = index_deposits(
        &terms,
        &authority.shielded,
        &env.assets()?,
        client,
        INDEX_TIMEOUT,
    )?;
    let deposit = |leg: Leg| -> Result<Deposit> {
        deposits
            .iter()
            .find(|deposit| deposit.leg == leg)
            .cloned()
            .ok_or_else(|| anyhow!("no {leg:?} deposit"))
    };
    let delivery = deposit(Leg::Delivery)?;
    let payment = deposit(Leg::Payment)?;

    // A settle of the same DvP, proven while it is open.
    let settle_ix = SettleDvpTransactionParams {
        terms: terms.clone(),
        current_timestamp: cluster_time(client)? - TIMESTAMP_LAG,
        dvp_utxo: discovered.dvp_utxo.clone(),
        dvp_utxo_leaf_index: discovered.leaf_index,
        delivery_deposit: delivery.clone(),
        payment_deposit: payment.clone(),
        payer: env.payer.pubkey(),
        tree_id: env.localnet.tree_id,
    }
    .build(&authority.shielded)?
    .prove(client.indexer(), tree)?
    .instruction()?;

    let cancel = prove_abort(
        &env,
        authority,
        AbortMode::Cancel,
        &discovered,
        Some(delivery.clone()),
        Some(payment.clone()),
    )?;
    ensure!(
        matches!(cancel, AbortDvp::Cancel(_)),
        "a cancel proves to a reject"
    );
    let cancel_ix = cancel.instruction()?;

    // Bypassing the sdk, the buyer submits the proven cancel as its own
    // signer: the proof binds the settlement authority's identity.
    let mut forged_ix = cancel_ix.clone();
    forged_ix
        .accounts
        .get_mut(1)
        .ok_or_else(|| anyhow!("cancel has no settlement authority account"))?
        .pubkey = env.buyer.solana.pubkey();
    assert_dvp_error(
        "the buyer cancelling",
        env.send(forged_ix, &[&env.buyer.solana], None),
        DvpError::ProofVerificationFailed,
    );

    let cancel_signature = env.send(cancel_ix, &[&authority.solana], None)?;
    client.confirm_private_transaction_sync(cancel_signature)?;

    // The settle fails on the spent DvP UTXO, not on its timestamp, which is
    // still in the window.
    let settle_error = match env.send(settle_ix, &[&authority.solana], None) {
        Ok(signature) => bail!("settle after cancel unexpectedly succeeded ({signature})"),
        Err(err) => format!("{err:?}"),
    };
    let timestamp_code = DvpError::TimestampOutOfWindow as u32;
    ensure!(
        !settle_error.contains(&format!("Custom({timestamp_code})")),
        "settle after cancel left the timestamp window: {settle_error}"
    );

    assert_eq!(
        refunds(&env, &terms, cancel_signature)?,
        (DELIVERY_AMOUNT + DELIVERY_SURPLUS, PAYMENT_AMOUNT)
    );
    for nullifier in [
        discovered.dvp_utxo.nullifier()?,
        delivery.to_input_utxo()?.nullifier,
        payment.to_input_utxo()?.nullifier,
    ] {
        wait_for_nullifier_present(client, tree, nullifier);
    }
    Ok(())
}

/// Create a DvP and fund both legs, then let the buyer reject it knowing
/// only its own payment deposit: the payment deposit fills input slot 1
/// (leg-order bit `PaymentFirst`, D3) and comes back in full, the seller gets
/// a zero-amount delivery refund (D2), and the DvP is closed. The seller's
/// delivery deposit, which the abort did not spend, stays reclaimable.
#[test]
fn buyer_rejects_with_only_the_payment_deposit() -> Result<()> {
    let env = setup(17)?;
    let client = &env.localnet.client;
    let tree = env.localnet.tree;
    let registry = env.assets()?;
    let (terms, discovered) = create(&env, &env.buyer)?;
    fund_both(&env, &terms, DELIVERY_AMOUNT)?;

    let own = index_own_deposits(
        &terms,
        &env.buyer.shielded,
        &registry,
        client,
        INDEX_TIMEOUT,
    )?;
    let [payment] = own.as_slice() else {
        bail!("the buyer discovered {} own deposits", own.len());
    };
    let reject = prove_abort(
        &env,
        &env.buyer,
        AbortMode::Reject,
        &discovered,
        None,
        Some(payment.clone()),
    )?;
    ensure!(
        matches!(reject, AbortDvp::Reject(_)),
        "a reject proves to a cancel"
    );
    let reject_signature = env.send(reject.instruction()?, &[&env.buyer.solana], None)?;
    client.confirm_private_transaction_sync(reject_signature)?;

    assert_eq!(
        refunds(&env, &terms, reject_signature)?,
        (0, PAYMENT_AMOUNT)
    );
    for nullifier in [
        discovered.dvp_utxo.nullifier()?,
        payment.to_input_utxo()?.nullifier,
    ] {
        wait_for_nullifier_present(client, tree, nullifier);
    }

    // After the close, the seller finds its delivery deposit and reclaims
    // it.
    let own = index_own_deposits(
        &terms,
        &env.seller.shielded,
        &registry,
        client,
        INDEX_TIMEOUT,
    )?;
    let [delivery] = own.as_slice() else {
        bail!("the seller discovered {} own deposits", own.len());
    };
    let reclaim_ix = ReclaimDvpTransactionParams {
        deposit: delivery.clone(),
        payer: env.payer.pubkey(),
        refund_destination: None,
        tree_id: env.localnet.tree_id,
    }
    .build(&env.seller.shielded)?
    .prove(client.indexer(), tree)?
    .instruction()?;
    let reclaim_signature = env.send(reclaim_ix, &[&env.seller.solana], None)?;
    client.confirm_private_transaction_sync(reclaim_signature)?;
    let refund = env.received_note(
        &env.seller,
        env.seller.address()?.confidential_view_tag()?,
        terms.delivery_mint,
        reclaim_signature,
    )?;
    assert_eq!(refund.utxo.amount, DELIVERY_AMOUNT);
    wait_for_nullifier_present(client, tree, delivery.to_input_utxo()?.nullifier);
    Ok(())
}

/// Create a DvP nobody funds and let the seller reject it: both input slots
/// are padding, both refunds are zero-amount, and the DvP UTXO is spent.
#[test]
fn seller_rejects_without_deposits() -> Result<()> {
    let env = setup(18)?;
    let client = &env.localnet.client;
    let (terms, discovered) = create(&env, &env.seller)?;

    let reject = prove_abort(
        &env,
        &env.seller,
        AbortMode::Reject,
        &discovered,
        None,
        None,
    )?;
    let reject_signature = env.send(reject.instruction()?, &[&env.seller.solana], None)?;
    client.confirm_private_transaction_sync(reject_signature)?;

    assert_eq!(refunds(&env, &terms, reject_signature)?, (0, 0));
    wait_for_nullifier_present(client, env.localnet.tree, discovered.dvp_utxo.nullifier()?);
    Ok(())
}

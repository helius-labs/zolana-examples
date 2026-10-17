mod shared;

use anyhow::{anyhow, bail, ensure, Result};
use dvp_program::error::DvpError;
use dvp_sdk::{
    index::{index_deposits, index_own_deposits, index_terms},
    instructions::{
        abort_dvp::{AbortDvp, AbortDvpTransactionParams, AbortMode},
        reclaim_dvp::ReclaimDvpTransactionParams,
        settle_dvp::SettleDvpTransactionParams,
    },
    state::{Deposit, Leg, SwapDvp},
};
use dvp_test::{assert_dvp_error, assert_spp_rejects, has_dvp, has_legs, settle_timestamp};
use shared::{setup, Actor, TestEnv, DELIVERY_AMOUNT, INDEX_TIMEOUT, PAYMENT_AMOUNT};
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

/// Cancel a funded DvP and reject stale or unauthorized proofs.
#[test]
fn authority_cancels_with_both_deposits() -> Result<()> {
    let env = setup(16)?;
    let client = &env.localnet.client;
    let tree = env.localnet.tree;
    let authority = &env.settlement_authority;
    // 1. Create the DvP and discover its terms.
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;
    let discovered = index_terms(&authority.shielded, client, INDEX_TIMEOUT, has_dvp(&terms))?;
    let [discovered] = discovered.as_slice() else {
        bail!("expected one DvP");
    };
    // 2. Fund delivery and payment.
    let delivery_utxo = env.shield(&env.seller, terms.delivery_mint, SHIELD_AMOUNT)?;
    let payment_utxo = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    env.fund(
        &env.seller,
        &terms,
        Leg::Delivery,
        DELIVERY_AMOUNT + DELIVERY_SURPLUS,
        delivery_utxo,
    )?;
    env.fund(
        &env.buyer,
        &terms,
        Leg::Payment,
        PAYMENT_AMOUNT,
        payment_utxo,
    )?;

    let deposits = index_deposits(
        &terms,
        &authority.shielded,
        &env.assets()?,
        client,
        INDEX_TIMEOUT,
        has_legs([Leg::Delivery, Leg::Payment]),
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

    // 3. Build and prove the cancel transaction.
    let cancel = AbortDvpTransactionParams {
        terms: terms.clone(),
        mode: AbortMode::Cancel,
        dvp_utxo: discovered.dvp_utxo.clone(),
        dvp_utxo_leaf_index: discovered.leaf_index,
        delivery_mint: Mint::new(fixture::spl_mint(), fixture::SPL_ASSET_ID),
        payment_mint: Mint::SOL,
        delivery_deposit: Some(delivery.clone()),
        payment_deposit: Some(payment.clone()),
        payer: env.payer.pubkey(),
        tree_id: env.localnet.tree_id,
    }
    .build(&authority.shielded)?
    .prove(client.indexer(), env.localnet.tree)?;
    ensure!(
        matches!(cancel, AbortDvp::Cancel(_)),
        "a cancel proves to a reject"
    );
    let cancel_ix = cancel.instruction()?;

    // 4. Check the buyer cannot submit the authority’s cancel proof.
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

    // 5. Check a cancel proof cannot authorize reject_dvp.
    let mut retagged_ix = cancel_ix.clone();
    *retagged_ix
        .data
        .first_mut()
        .ok_or_else(|| anyhow!("cancel has no instruction tag"))? = dvp_program::tag::REJECT_DVP;
    assert_dvp_error(
        "a cancel proof sent as a reject",
        env.send(retagged_ix, &[&authority.solana], None),
        DvpError::ProofVerificationFailed,
    );

    // 6. Prove settlement before cancellation.
    let settle_ix = SettleDvpTransactionParams {
        terms: terms.clone(),
        current_timestamp: settle_timestamp(client)?,
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

    let cancel_signature = env.send(cancel_ix, &[&authority.solana], None)?;
    client.confirm_private_transaction_sync(cancel_signature)?;

    // 7. Check settlement fails after cancellation.
    assert_spp_rejects(
        "settle after cancel",
        env.send(settle_ix, &[&authority.solana], None),
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

/// Reject with payment only, then reclaim the omitted delivery deposit.
#[test]
fn buyer_rejects_with_only_the_payment_deposit() -> Result<()> {
    let env = setup(17)?;
    let client = &env.localnet.client;
    let tree = env.localnet.tree;
    let registry = env.assets()?;
    // 1. Create the DvP and discover its terms.
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;
    let discovered = index_terms(&env.buyer.shielded, client, INDEX_TIMEOUT, has_dvp(&terms))?;
    let [discovered] = discovered.as_slice() else {
        bail!("expected one DvP");
    };
    // 2. Fund delivery and payment.
    let delivery_utxo = env.shield(&env.seller, terms.delivery_mint, SHIELD_AMOUNT)?;
    let payment_utxo = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    env.fund(
        &env.seller,
        &terms,
        Leg::Delivery,
        DELIVERY_AMOUNT,
        delivery_utxo,
    )?;
    env.fund(
        &env.buyer,
        &terms,
        Leg::Payment,
        PAYMENT_AMOUNT,
        payment_utxo,
    )?;

    let own = index_own_deposits(
        &terms,
        &env.buyer.shielded,
        &registry,
        client,
        INDEX_TIMEOUT,
        has_legs([Leg::Payment]),
    )?;
    let [payment] = own.as_slice() else {
        bail!("the buyer discovered {} own deposits", own.len());
    };
    // 3. Build and prove the reject transaction.
    let reject = AbortDvpTransactionParams {
        terms: terms.clone(),
        mode: AbortMode::Reject,
        dvp_utxo: discovered.dvp_utxo.clone(),
        dvp_utxo_leaf_index: discovered.leaf_index,
        delivery_mint: Mint::new(fixture::spl_mint(), fixture::SPL_ASSET_ID),
        payment_mint: Mint::SOL,
        delivery_deposit: None,
        payment_deposit: Some(payment.clone()),
        payer: env.payer.pubkey(),
        tree_id: env.localnet.tree_id,
    }
    .build(&env.buyer.shielded)?
    .prove(client.indexer(), env.localnet.tree)?;
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

    // 4. Reclaim the delivery deposit after rejection.
    let own = index_own_deposits(
        &terms,
        &env.seller.shielded,
        &registry,
        client,
        INDEX_TIMEOUT,
        has_legs([Leg::Delivery]),
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
    let refund = env.received_utxo(
        &env.seller,
        env.seller.address()?.confidential_view_tag()?,
        terms.delivery_mint,
        reclaim_signature,
    )?;
    assert_eq!(refund.utxo.amount, DELIVERY_AMOUNT);
    wait_for_nullifier_present(client, tree, delivery.to_input_utxo()?.nullifier);
    Ok(())
}

/// Reject an unfunded DvP and check both real zero refunds.
#[test]
fn seller_rejects_without_deposits() -> Result<()> {
    let env = setup(18)?;
    let client = &env.localnet.client;
    // 1. Create the DvP and discover its terms.
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;
    let discovered = index_terms(&env.seller.shielded, client, INDEX_TIMEOUT, has_dvp(&terms))?;
    let [discovered] = discovered.as_slice() else {
        bail!("expected one DvP");
    };

    // 2. Build and prove the reject transaction.
    let reject = AbortDvpTransactionParams {
        terms: terms.clone(),
        mode: AbortMode::Reject,
        dvp_utxo: discovered.dvp_utxo.clone(),
        dvp_utxo_leaf_index: discovered.leaf_index,
        delivery_mint: Mint::new(fixture::spl_mint(), fixture::SPL_ASSET_ID),
        payment_mint: Mint::SOL,
        delivery_deposit: None,
        payment_deposit: None,
        payer: env.payer.pubkey(),
        tree_id: env.localnet.tree_id,
    }
    .build(&env.seller.shielded)?
    .prove(client.indexer(), env.localnet.tree)?;
    let reject_signature = env.send(reject.instruction()?, &[&env.seller.solana], None)?;
    client.confirm_private_transaction_sync(reject_signature)?;

    assert_eq!(refunds(&env, &terms, reject_signature)?, (0, 0));
    wait_for_nullifier_present(client, env.localnet.tree, discovered.dvp_utxo.nullifier()?);
    Ok(())
}

/// Read the two refund amounts from the parties' wallets.
fn refunds(env: &TestEnv, terms: &SwapDvp, signature: Signature) -> Result<(u64, u64)> {
    let refund = |actor: &Actor, mint| -> Result<u64> {
        Ok(env
            .received_utxo(
                actor,
                actor.address()?.confidential_view_tag()?,
                mint,
                signature,
            )?
            .utxo
            .amount)
    };
    Ok((
        refund(&env.seller, terms.delivery_mint)?,
        refund(&env.buyer, terms.payment_mint)?,
    ))
}

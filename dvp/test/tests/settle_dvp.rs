mod shared;

use anyhow::{anyhow, bail, ensure, Result};
use dvp_program::error::DvpError;
use dvp_sdk::{
    index::{index_deposits, index_own_deposits, index_terms},
    instructions::{
        reclaim_dvp::ReclaimDvpTransactionParams, settle_dvp::SettleDvpTransactionParams,
    },
    state::{Deposit, Leg},
};
use dvp_test::{assert_dvp_error, assert_spp_rejects, has_dvp, has_legs, settle_timestamp};
use shared::{setup, DELIVERY_AMOUNT, INDEX_TIMEOUT, PAYMENT_AMOUNT};
use solana_signer::Signer;
use zolana_test_utils::test_validator_asserts::wait_for_nullifier_present;
use zolana_transaction::SOL_MINT;

/// What the seller sends above the delivery amount, returned to its refund
/// destination at settlement.
const DELIVERY_SURPLUS: u64 = 50_000_000;
/// What each funder shields into its private balance before funding.
const SHIELD_AMOUNT: u64 = 600_000_000;

/// Settle both legs with surplus and reclaim a late deposit.
#[test]
fn settle_dvp_full_flow() -> Result<()> {
    let env = setup(15)?;
    let client = &env.localnet.client;
    let tree = env.localnet.tree;
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;
    let authority = &env.settlement_authority;
    let discovered = index_terms(&authority.shielded, client, INDEX_TIMEOUT, has_dvp(&terms))?;
    let [discovered] = discovered.as_slice() else {
        bail!(
            "the settlement authority discovered {} DvPs",
            discovered.len()
        );
    };

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

    let registry = env.assets()?;
    let deposits = index_deposits(
        &terms,
        &authority.shielded,
        &registry,
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

    // 1. Prove settlement against a recent cluster timestamp.
    let current_timestamp = settle_timestamp(client)?;
    let settle_ix = SettleDvpTransactionParams {
        terms: terms.clone(),
        current_timestamp,
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

    // 2. Check the buyer cannot submit the authority’s proof.
    let mut forged_ix = settle_ix.clone();
    forged_ix
        .accounts
        .get_mut(1)
        .ok_or_else(|| anyhow!("settle has no settlement authority account"))?
        .pubkey = env.buyer.solana.pubkey();
    assert_dvp_error(
        "the buyer settling",
        env.send(forged_ix, &[&env.buyer.solana], None),
        DvpError::ProofVerificationFailed,
    );

    let settle_signature = env.send(settle_ix.clone(), &[&authority.solana], None)?;
    client.confirm_private_transaction_sync(settle_signature)?;

    // 3. Check settlement replay fails on spent inputs.
    assert_spp_rejects(
        "second settle",
        env.send(settle_ix, &[&authority.solana], Some(1)),
    );

    // 4. Check the parties receive settlement and change.
    let settled = |actor, mint| -> Result<(_, u64)> {
        let utxo = env.received_utxo(
            actor,
            actor.address()?.confidential_view_tag()?,
            mint,
            settle_signature,
        )?;
        Ok((utxo.utxo.asset.asset, utxo.utxo.amount))
    };
    assert_eq!(
        (
            settled(&env.buyer, terms.delivery_mint)?,
            settled(&env.seller, terms.payment_mint)?,
            settled(&env.seller, terms.delivery_mint)?,
            settled(&env.buyer, terms.payment_mint)?,
        ),
        (
            (terms.delivery_mint, DELIVERY_AMOUNT),
            (terms.payment_mint, PAYMENT_AMOUNT),
            (terms.delivery_mint, DELIVERY_SURPLUS),
            (terms.payment_mint, 0),
        )
    );

    // 5. Check the DvP UTXO and both deposits are spent.
    for nullifier in [
        discovered.dvp_utxo.nullifier()?,
        delivery.to_input_utxo()?.nullifier,
        payment.to_input_utxo()?.nullifier,
    ] {
        wait_for_nullifier_present(client, tree, nullifier);
    }

    // 6. Fund payment again after settlement.
    let late_utxo = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    env.fund(&env.buyer, &terms, Leg::Payment, PAYMENT_AMOUNT, late_utxo)?;
    let own = index_own_deposits(
        &terms,
        &env.buyer.shielded,
        &registry,
        client,
        INDEX_TIMEOUT,
        has_legs([Leg::Payment]),
    )?;
    // 7. Discover and reclaim the late payment deposit.
    let [late] = own.as_slice() else {
        bail!("the buyer discovered {} unspent own deposits", own.len());
    };
    ensure!(late != &payment, "the settled payment deposit is unspent");
    let reclaim_ix = ReclaimDvpTransactionParams {
        deposit: late.clone(),
        payer: env.payer.pubkey(),
        refund_destination: None,
        tree_id: env.localnet.tree_id,
    }
    .build(&env.buyer.shielded)?
    .prove(client.indexer(), tree)?
    .instruction()?;
    let reclaim_signature = env.send(reclaim_ix, &[&env.buyer.solana], None)?;
    client.confirm_private_transaction_sync(reclaim_signature)?;
    let refund = env.received_utxo(
        &env.buyer,
        env.buyer.address()?.confidential_view_tag()?,
        SOL_MINT,
        reclaim_signature,
    )?;
    assert_eq!(refund.utxo.amount, PAYMENT_AMOUNT);
    wait_for_nullifier_present(client, tree, late.to_input_utxo()?.nullifier);
    Ok(())
}

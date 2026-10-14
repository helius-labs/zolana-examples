mod shared;

use anyhow::{anyhow, bail, ensure, Result};
use dvp_program::{error::DvpError, DVP_ADDRESS_TREE_ID};
use dvp_sdk::{
    index::{index_deposits, index_own_deposits, index_terms, DiscoveredDvp},
    state::{Deposit, DvpUtxo, Leg},
};
use shared::{assert_dvp_error, setup, DELIVERY_AMOUNT, INDEX_TIMEOUT, PAYMENT_AMOUNT};
use zolana_transaction::SOL_MINT;

/// What the seller sends above the delivery amount, refunded at settlement.
const DELIVERY_SURPLUS: u64 = 50_000_000;
/// What each funder shields into its private balance before funding; the
/// rest of the note returns as change.
const SHIELD_AMOUNT: u64 = 600_000_000;

#[test]
fn create_dvp_and_discover_terms() -> Result<()> {
    let env = setup(11)?;
    // D6: the create circuit pins the address slot to tree 0, so the
    // localnet tree the address is proven against must be tree 0.
    assert_eq!(env.localnet.tree_id, DVP_ADDRESS_TREE_ID);
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    let dvp_utxo_hash = create
        .spp
        .outputs
        .first()
        .map(|output| output.utxo_hash)
        .ok_or_else(|| anyhow::anyhow!("create transact has no output"))?;
    let create_ix = create.instruction()?;
    let create_signature = env.send(create_ix.clone(), &[&env.seller.solana], None)?;

    // Each party finds its message by its own pubkey, decrypts the terms and
    // recomputes the DvP UTXO the transaction committed.
    let expected_utxo = DvpUtxo::new(terms.clone(), env.localnet.tree_id);
    assert_eq!(expected_utxo.hash()?, dvp_utxo_hash);
    for (recipient_index, party) in (0u32..).zip(env.parties()) {
        let found = index_terms(&party.shielded, &env.localnet.client, INDEX_TIMEOUT)?;
        let [discovered] = found.as_slice() else {
            anyhow::bail!("party {recipient_index} discovered {} DvPs", found.len());
        };
        assert_eq!(
            discovered,
            &DiscoveredDvp {
                dvp_utxo: expected_utxo.clone(),
                leaf_index: discovered.leaf_index,
                recipient_index,
                tx_signature: create_signature,
            }
        );
    }

    // The address is indexed now, so no second proof of its non-inclusion
    // exists: resend the same signed instruction instead, made distinct by a
    // priority fee. Only failure is asserted, since the SPP error depends on
    // whether the nullifier is still queued.
    ensure!(
        env.send(create_ix, &[&env.seller.solana], Some(1)).is_err(),
        "duplicate create unexpectedly succeeded"
    );
    Ok(())
}

#[test]
fn create_dvp_rejects_a_mismatched_dvp_id() -> Result<()> {
    let env = setup(12)?;
    let terms = env.terms(1)?;
    let other_terms = env.terms(2)?;
    let (other_pda, other_bump) = other_terms.dvp_authority()?;
    let mut create = env.prove_create(&terms)?;
    // The other terms' id with its own bump derives a valid PDA, but the
    // create proof binds the original id and owner hash.
    create.dvp_id = other_terms.dvp_id()?;
    create.bump = other_bump;
    ensure!(
        other_pda != terms.dvp_authority()?.0,
        "the two DvPs share a PDA"
    );
    assert_dvp_error(
        "create with a mismatched dvp_id",
        env.send(create.instruction()?, &[&env.seller.solana], None),
        DvpError::ProofVerificationFailed,
    );
    Ok(())
}

#[test]
fn fund_dvp_and_discover_deposits() -> Result<()> {
    let env = setup(13)?;
    let client = &env.localnet.client;
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;
    let authority = &env.settlement_authority.shielded;
    let discovered = index_terms(authority, client, INDEX_TIMEOUT)?;
    let [discovered] = discovered.as_slice() else {
        bail!(
            "the settlement authority discovered {} DvPs",
            discovered.len()
        );
    };
    assert_eq!(discovered.dvp_utxo.terms, terms);

    // The seller funds delivery (SPL, `transfer`) with a surplus; the buyer
    // funds payment (SOL, `transfer_sol`) exactly.
    let delivery_note = env.shield(&env.seller, terms.delivery_mint, SHIELD_AMOUNT)?;
    let payment_note = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    let delivery_amount = DELIVERY_AMOUNT + DELIVERY_SURPLUS;
    env.fund(
        &env.seller,
        &terms,
        Leg::Delivery,
        delivery_amount,
        delivery_note,
    )?;
    env.fund(
        &env.buyer,
        &terms,
        Leg::Payment,
        PAYMENT_AMOUNT,
        payment_note,
    )?;

    // The settlement authority finds both deposits by the PDA tag and reads
    // them with its viewing key.
    let registry = env.assets()?;
    let deposits = index_deposits(&terms, authority, &registry, client, INDEX_TIMEOUT)?;
    let deposit = |leg: Leg| -> Result<&Deposit> {
        deposits
            .iter()
            .find(|deposit| deposit.leg == leg)
            .ok_or_else(|| anyhow!("no {leg:?} deposit"))
    };
    let delivery = deposit(Leg::Delivery)?;
    let payment = deposit(Leg::Payment)?;
    assert_eq!(
        (
            deposits.len(),
            (delivery.mint.asset, delivery.amount),
            (payment.mint.asset, payment.amount),
        ),
        (
            2,
            (terms.delivery_mint, delivery_amount),
            (SOL_MINT, PAYMENT_AMOUNT),
        )
    );

    // Each funder re-reads exactly its own deposit through its transaction
    // viewing key, and sees the opening the authority sees.
    for (funder, expected) in [(&env.seller, delivery), (&env.buyer, payment)] {
        let own = index_own_deposits(&terms, &funder.shielded, &registry, client, INDEX_TIMEOUT)?;
        assert_eq!(own, vec![expected.clone()]);
    }
    Ok(())
}

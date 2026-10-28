mod shared;

use anyhow::{anyhow, bail, ensure, Result};
use dvp_program::{error::DvpError, DVP_ADDRESS_TREE_ID};
use dvp_sdk::{
    index::{index_deposits, index_own_deposits, index_terms, DiscoveredDvp},
    state::{Deposit, DvpUtxo, Leg},
};
use dvp_test::{assert_dvp_error, assert_spp_rejects, has_dvp, has_legs};
use shared::{setup, DELIVERY_AMOUNT, INDEX_TIMEOUT, PAYMENT_AMOUNT};
use zolana_transaction::SOL_MINT;

/// What the seller sends above the delivery amount, refunded at settlement.
const DELIVERY_SURPLUS: u64 = 50_000_000;
/// What each funder shields into its private balance before funding; the
/// rest of the utxo returns as change.
const SHIELD_AMOUNT: u64 = 600_000_000;

#[test]
fn create_dvp_and_discover_terms() -> Result<()> {
    let env = setup(11)?;
    // 1. Check the pinned address tree and create the DvP.
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

    // 2. Check all three parties discover the committed terms.
    let expected_utxo = DvpUtxo::new(terms.clone(), env.localnet.tree_id);
    assert_eq!(expected_utxo.hash()?, dvp_utxo_hash);
    for (recipient_index, party) in (0u32..).zip(env.parties()) {
        let found = index_terms(
            &party.shielded,
            &env.localnet.client,
            INDEX_TIMEOUT,
            has_dvp(&terms),
        )?;
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

    // 3. Check replay fails on the reserved address.
    assert_spp_rejects(
        "duplicate create",
        env.send(create_ix, &[&env.seller.solana], Some(1)),
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
    // 1. Check another DvP ID and valid PDA fail verification.
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
    let discovered = index_terms(authority, client, INDEX_TIMEOUT, has_dvp(&terms))?;
    let [discovered] = discovered.as_slice() else {
        bail!(
            "the settlement authority discovered {} DvPs",
            discovered.len()
        );
    };
    assert_eq!(discovered.dvp_utxo.terms, terms);

    // 1. Fund delivery with surplus and payment exactly.
    let delivery_utxo = env.shield(&env.seller, terms.delivery_mint, SHIELD_AMOUNT)?;
    let payment_utxo = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    let delivery_amount = DELIVERY_AMOUNT + DELIVERY_SURPLUS;
    env.fund(
        &env.seller,
        &terms,
        Leg::Delivery,
        delivery_amount,
        delivery_utxo,
    )?;
    env.fund(
        &env.buyer,
        &terms,
        Leg::Payment,
        PAYMENT_AMOUNT,
        payment_utxo,
    )?;

    // 2. Check the settlement authority decrypts both deposits.
    let registry = env.assets()?;
    let deposits = index_deposits(
        &terms,
        authority,
        &registry,
        client,
        INDEX_TIMEOUT,
        has_legs([Leg::Delivery, Leg::Payment]),
    )?;
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

    // 3. Check each funder decrypts only its own deposit.
    for (funder, expected) in [(&env.seller, delivery), (&env.buyer, payment)] {
        let own = index_own_deposits(
            &terms,
            &funder.shielded,
            &registry,
            client,
            INDEX_TIMEOUT,
            has_legs([expected.leg]),
        )?;
        assert_eq!(own, vec![expected.clone()]);
    }
    Ok(())
}

#[test]
#[ignore = "requires the Node WASM build and npm ci; run explicitly after the Rust flows"]
fn node_client_lifecycles() -> anyhow::Result<()> {
    use anyhow::ensure;
    use std::{path::Path, process::Command};
    use zolana_program_test::{fixture, localnet::LocalnetPorts};
    use zolana_transaction::SOL_MINT;
    let env = shared::setup(12)?;
    env.shield(&env.seller, fixture::spl_mint(), 900_000_000)?;
    env.shield(&env.buyer, SOL_MINT, 50_000_000)?;
    let ports = LocalnetPorts::for_test(12)?;
    let seed = |key: &solana_keypair::Keypair| -> String {
        key.to_bytes()
            .iter()
            .take(32)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    for flow in ["settle", "reclaim", "cancel", "reject"] {
        let status = Command::new("npm")
            .args(["run", "example", "--", &format!("examples/{flow}.ts")])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../typescript"))
            .env("ZOLANA_LOCALNET_URL", ports.rpc_url())
            .env("ZOLANA_INDEXER_URL", ports.photon_url())
            .env(
                "ZOLANA_PROVER_URL",
                std::env::var("ZOLANA_PROVER_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:3001".into()),
            )
            .env("DVP_SELLER_SEED", seed(&env.seller.solana))
            .env("DVP_BUYER_SEED", seed(&env.buyer.solana))
            .env("DVP_AUTHORITY_SEED", seed(&env.settlement_authority.solana))
            .env("DVP_DELIVERY_MINT", fixture::spl_mint().to_string())
            .env("DVP_DELIVERY_ASSET_ID", fixture::SPL_ASSET_ID.to_string())
            .status()?;
        ensure!(status.success(), "Node {flow} example failed: {status}");
    }
    Ok(())
}

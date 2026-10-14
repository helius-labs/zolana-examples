mod shared;

use anyhow::{anyhow, bail, ensure, Result};
use dvp_program::error::DvpError;
use dvp_sdk::{
    index::{index_deposits, index_own_deposits},
    instructions::reclaim_dvp::ReclaimDvpTransactionParams,
    state::{Deposit, DvpUtxo, Leg},
};
use dvp_test::{assert_dvp_error, assert_spp_rejects, failure_text, has_legs};
use shared::{setup, DELIVERY_AMOUNT, INDEX_TIMEOUT, PAYMENT_AMOUNT};
use solana_signer::Signer;
use zolana_client::Rpc;
use zolana_test_utils::{
    nullifier_pda::assert_nullifier_pdas_absent, test_validator_asserts::wait_for_nullifier_present,
};
use zolana_transaction::SOL_MINT;

/// What each funder shields into its private balance before funding.
const SHIELD_AMOUNT: u64 = 600_000_000;

/// Create a DvP, fund both legs, and let the seller reclaim its delivery
/// deposit while the DvP is open: the refund lands in the seller's wallet,
/// the deposit is spent, and the DvP UTXO and the payment deposit are not.
/// The buyer cannot reclaim the seller's deposit, the proven reclaim does not
/// run against other SPP accounts, and the deposit cannot be reclaimed
/// twice.
#[test]
fn reclaim_dvp_while_open() -> Result<()> {
    let env = setup(14)?;
    let client = &env.localnet.client;
    let tree = env.localnet.tree;
    let terms = env.terms(1)?;
    let create = env.prove_create(&terms)?;
    env.send(create.instruction()?, &[&env.seller.solana], None)?;

    let delivery_note = env.shield(&env.seller, terms.delivery_mint, SHIELD_AMOUNT)?;
    let payment_note = env.shield(&env.buyer, SOL_MINT, SHIELD_AMOUNT)?;
    env.fund(
        &env.seller,
        &terms,
        Leg::Delivery,
        DELIVERY_AMOUNT,
        delivery_note,
    )?;
    env.fund(
        &env.buyer,
        &terms,
        Leg::Payment,
        PAYMENT_AMOUNT,
        payment_note,
    )?;

    // The seller re-reads its own deposit through its transaction viewing
    // key; the settlement authority sees the payment deposit.
    let registry = env.assets()?;
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
    let deposits = index_deposits(
        &terms,
        &env.settlement_authority.shielded,
        &registry,
        client,
        INDEX_TIMEOUT,
        has_legs([Leg::Payment]),
    )?;
    let payment: &Deposit = deposits
        .iter()
        .find(|deposit| deposit.leg == Leg::Payment)
        .ok_or_else(|| anyhow!("no payment deposit"))?;

    // Refunds go to the signer's own address.
    let reclaim = || ReclaimDvpTransactionParams {
        deposit: delivery.clone(),
        payer: env.payer.pubkey(),
        refund_destination: None,
        tree_id: env.localnet.tree_id,
    };

    // The sdk refuses to build the buyer's reclaim of the delivery deposit:
    // the reclaim circuit ties the delivery leg to the seller.
    ensure!(
        reclaim().build(&env.buyer.shielded).is_err(),
        "the sdk built the buyer's reclaim of the delivery deposit"
    );

    let reclaim_ix = reclaim()
        .build(&env.seller.shielded)?
        .prove(client.indexer(), tree)?
        .instruction()?;

    // Bypassing the sdk, the buyer submits the seller's proven reclaim as
    // its own signer: the proof binds the seller's identity.
    let mut forged_ix = reclaim_ix.clone();
    forged_ix
        .accounts
        .get_mut(1)
        .ok_or_else(|| anyhow!("reclaim has no signer account"))?
        .pubkey = env.buyer.solana.pubkey();
    assert_dvp_error(
        "the buyer reclaiming the delivery deposit",
        env.send(forged_ix, &[&env.buyer.solana], None),
        DvpError::ProofVerificationFailed,
    );

    // The proof does not bind the forwarded SPP accounts; the program checks
    // them after the proof, before its CPI. Account 4 is the SPP program and
    // the last one the dvp_authority PDA.
    let mut wrong_program_ix = reclaim_ix.clone();
    wrong_program_ix
        .accounts
        .get_mut(4)
        .ok_or_else(|| anyhow!("reclaim has no SPP program account"))?
        .pubkey = env.buyer.solana.pubkey();
    assert_dvp_error(
        "a reclaim forwarded to another program",
        env.send(wrong_program_ix, &[&env.seller.solana], None),
        DvpError::InvalidShieldedPoolProgram,
    );
    let mut no_pda_ix = reclaim_ix.clone();
    no_pda_ix.accounts.pop();
    assert_dvp_error(
        "a reclaim without the dvp_authority account",
        env.send(no_pda_ix, &[&env.seller.solana], None),
        DvpError::MissingDvpAuthority,
    );
    let mut short_ix = reclaim_ix.clone();
    short_ix.accounts.truncate(4);
    let short_error = failure_text(
        "a reclaim with two SPP accounts",
        env.send(short_ix, &[&env.seller.solana], None),
    );
    // `ProgramError::NotEnoughAccountKeys`, as the RPC error prints it.
    ensure!(
        short_error.contains("insufficient account keys for instruction"),
        "a reclaim with two SPP accounts: expected NotEnoughAccountKeys in: {short_error}"
    );

    let reclaim_signature = env.send(reclaim_ix.clone(), &[&env.seller.solana], None)?;
    client.confirm_private_transaction_sync(reclaim_signature)?;

    // The refund is a spendable note of the whole deposit in the seller's
    // wallet.
    let refund = env.received_note(
        &env.seller,
        env.seller.address()?.confidential_view_tag()?,
        terms.delivery_mint,
        reclaim_signature,
    )?;
    assert_eq!(
        (refund.utxo.asset.asset, refund.utxo.amount),
        (terms.delivery_mint, DELIVERY_AMOUNT)
    );

    // The deposit is spent; the DvP UTXO and the payment deposit are not,
    // so the DvP stays open.
    let delivery_nullifier = delivery.to_input_utxo()?.nullifier;
    wait_for_nullifier_present(client, tree, delivery_nullifier);
    let unspent = [
        DvpUtxo::new(terms.clone(), env.localnet.tree_id).nullifier()?,
        payment.to_input_utxo()?.nullifier,
    ];
    let proofs = client.get_non_inclusion_proofs(tree, unspent.to_vec(), None)?;
    assert_eq!(proofs.proofs.len(), unspent.len());
    assert_nullifier_pdas_absent(client, &tree, &unspent)?;

    // A second reclaim of the same deposit fails in SPP: its nullifier
    // exists.
    assert_spp_rejects(
        "second reclaim of the delivery deposit",
        env.send(reclaim_ix, &[&env.seller.solana], Some(1)),
    );
    Ok(())
}

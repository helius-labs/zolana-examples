mod shared;

use std::time::Duration;

use anyhow::{anyhow, Result};
use shared::{send, setup, TestEnv, DESTINATION_AMOUNT, SOURCE_AMOUNT};
use spr_client::{user_registry::ensure_registered, Rpc};
use spr_interface::error::ShieldedPoolError;
use spr_test_utils::{test_validator_asserts::wait_for_merkle_proof, wallet::Filter};
use spr_transaction::{Mint, SOL_MINT};
use swap_sdk::{
    index::index_taker,
    instructions::{
        make::MakeTransactionParams,
        take::{Take, TakeTransactionParams, DESTINATION_OUTPUT_SLOT, SOURCE_OUTPUT_SLOT},
    },
    shared::identity,
    state::OrderTerms,
};

const EXPIRY: u64 = 2_000_000_000;

// Confidential SPL<->SOL swap on the shielded pool -- make then take --
// driven against a real localnet (validator + Photon indexer + prover) that
// `setup()` starts.
//
// Flow:
//   1. Fund (in setup): 1.0 SPL is shielded to the maker, the taker shields
//      0.25 SOL.
//   2. Make: the maker spends its 1.0 SPL UTXO -> change 0.6 SPL (back to the
//      maker) + order UTXO 0.4 SPL (owned by the order-authority PDA,
//      designated to the taker and encrypted to it), plus the taker's marker.
//   3. Discover: the taker finds the marker, decrypts the order and checks the
//      maker it names against the user registry.
//   4. Take: the taker spends the order UTXO + its 0.25 SOL UTXO -> source
//      0.4 SPL (to the taker) + destination 0.25 SOL (to the maker). The
//      taker withholds the maker payout ciphertext; the maker still derives
//      the payout from the order opening and the first nullifier.
//      A take naming another maker fails first: the program signs for that
//      maker's order authority, which does not own the order UTXO, so SPP
//      rejects the transact proof.
//   5. Assert both take outputs are indexed.
//
// Net: maker 1.0 SPL -> 0.6 SPL + 0.25 SOL; taker 0.25 SOL -> 0.4 SPL.
#[test]
fn make_and_take_swap_inline() -> Result<()> {
    let TestEnv {
        localnet,
        maker,
        maker_input,
        taker,
        spl_mint,
    } = setup(2)?;
    let client = &localnet.client;
    let maker_address = maker.keypair.shielded_address()?;
    let maker_pubkey = maker_address.solana_address()?;
    let taker_address = taker.keypair.shielded_address()?;
    let taker_pubkey = taker_address.solana_address()?;
    ensure_registered(client.rpc(), &maker.keypair, &maker.keypair)
        .map_err(|e| anyhow!("register maker: {e:?}"))?;

    // 1. Make an order designated to the taker.
    let terms = OrderTerms {
        destination_mint: Mint::SOL,
        destination_amount: DESTINATION_AMOUNT,
        maker: maker_address,
        expiry: EXPIRY,
        taker: Some(identity(&taker_pubkey)?),
    };
    assert_eq!(maker_input.utxo.asset, maker.registry.mint(&spl_mint)?);
    let make_ix = MakeTransactionParams {
        terms,
        source_amount: SOURCE_AMOUNT,
        source: maker_input,
        taker_address,
        payer: maker_pubkey,
    }
    .build(&maker.keypair)?
    .prove(client.indexer(), localnet.tree, &maker.keypair)?
    .instruction()?;
    let make_signature = send(client.rpc(), &maker.keypair, make_ix)?;
    client
        .confirm_private_transaction_sync(make_signature)
        .map_err(|e| anyhow!("confirm make indexed: {e:?}"))?;

    // 2. Discover the order from the taker's side.
    let order = index_taker(
        &taker.keypair,
        &taker.registry,
        client.indexer(),
        client.rpc(),
        Duration::from_secs(60),
    )?
    .pop()
    .ok_or_else(|| anyhow!("no swap order discovered"))?;
    assert_eq!(order.maker_pubkey, maker_pubkey);
    let order_utxo = order.order_utxo;

    // 3. Take with the taker's SOL note.
    let taker_input = taker
        .balance(SOL_MINT, Some(Filter::MinAmount(DESTINATION_AMOUNT)))?
        .utxos
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no spendable SOL note of {DESTINATION_AMOUNT}"))?;
    let order_state = wait_for_merkle_proof(
        client.indexer(),
        localnet.tree,
        order_utxo.hash(localnet.tree_id)?,
    );
    let mut take_transaction = TakeTransactionParams {
        order_utxo: order_utxo.clone(),
        order_leaf_index: order_state.leaf_index,
        taker_input,
        payer: taker_pubkey,
        tree_id: localnet.tree_id,
    }
    .build(&taker.keypair)?;

    // 4. Withhold the maker payout ciphertext; the maker derives the payout
    // from the order opening and the published first nullifier.
    let outputs = &mut take_transaction.spp_proof_inputs.external_data.outputs;
    let source_output_hash = outputs
        .get(SOURCE_OUTPUT_SLOT)
        .ok_or_else(|| anyhow!("missing source output"))?
        .utxo_hash;
    let payout = outputs
        .get_mut(DESTINATION_OUTPUT_SLOT)
        .ok_or_else(|| anyhow!("missing maker payout"))?;
    payout.data = None;
    let destination_output_hash = payout.utxo_hash;
    let recovered = order_utxo.derived_destination_output(&take_transaction.first_nullifier)?;
    assert_eq!(recovered.hash(localnet.tree_id)?, destination_output_hash);

    let take = take_transaction.prove(client.indexer(), localnet.tree, &taker.keypair)?;
    assert_eq!(take.maker, maker_pubkey);
    let wrong_maker_ix = Take {
        maker: taker_pubkey,
        ..take.clone()
    }
    .instruction()?;
    // SPP checks the transact proof against its owner signers, the other
    // maker's order authority instead of the order UTXO's owner.
    let error = send(client.rpc(), &taker.keypair, wrong_maker_ix)
        .expect_err("a take naming another maker must fail");
    let spp_error = ShieldedPoolError::TransactProofVerificationFailed as u32;
    assert!(
        format!("{error:#}").contains(&format!("custom program error: {spp_error:#x}")),
        "expected TransactProofVerificationFailed: {error:#}"
    );
    let take_ix = take.instruction()?;
    let take_signature = send(client.rpc(), &taker.keypair, take_ix)?;
    client
        .confirm_private_transaction_sync(take_signature)
        .map_err(|e| anyhow!("confirm take indexed: {e:?}"))?;

    // 5. Both take outputs are indexed.
    client
        .indexer()
        .get_merkle_proofs(
            localnet.tree,
            vec![source_output_hash, destination_output_hash],
            None,
        )
        .map_err(|e| anyhow!("take outputs index: {e}"))?;
    Ok(())
}

mod shared;

use std::time::Duration;

use anyhow::{anyhow, Result};
use shared::{send, setup, TestEnv, DESTINATION_AMOUNT, SOURCE_AMOUNT};
use spr_client::{user_registry::ensure_registered, Rpc};
use spr_test_utils::test_validator_asserts::wait_for_merkle_proof;
use spr_transaction::{Mint, SppProofOutputUtxo};
use swap_sdk::{
    index::index_maker,
    instructions::{cancel::CancelTransactionParams, make::MakeTransactionParams},
    state::OrderTerms,
};

// The committed order expiry is already in the past, so the maker can cancel
// immediately: the swap program requires `now > order_expiry`.
const EXPIRY: u64 = 1_000_000;

// Confidential swap cancel on the shielded pool -- make then cancel -- driven
// against the same localnet (validator + Photon indexer + prover) as swap.rs.
//
// Flow:
//   1. Fund (in setup): 1.0 SPL is shielded to the order authority.
//   2. Make: as in swap.rs, but an open order (no designated taker) whose
//      expiry is already in the past.
//   3. Discover: the maker rediscovers the order opening from the indexer
//      (`index_maker`), decrypting the order UTXO slot from the sender side.
//   4. Cancel: the maker spends the order UTXO (0.4 SPL) -> source output
//      0.4 SPL back to the maker.
//   5. Assert the returned source output is indexed.
//
// Net: maker 1.0 SPL -> 0.6 SPL change + 0.4 SPL returned; the taker never acts.
#[test]
fn make_and_cancel_swap_inline() -> Result<()> {
    let TestEnv {
        localnet,
        maker,
        maker_input,
        taker,
        spl_mint,
    } = setup(1)?;
    let client = &localnet.client;
    let maker_address = maker.keypair.shielded_address()?;
    let maker_pubkey = maker_address.solana_address()?;
    ensure_registered(client.rpc(), &maker.keypair, &maker.keypair)
        .map_err(|e| anyhow!("register maker: {e:?}"))?;

    // 1. Make an open order that has already expired.
    assert_eq!(maker_input.utxo.asset.asset, spl_mint);
    let terms = OrderTerms {
        destination_mint: Mint::SOL,
        destination_amount: DESTINATION_AMOUNT,
        maker: maker_address,
        expiry: EXPIRY,
        taker: None,
    };
    let make_ix = MakeTransactionParams {
        terms,
        source_amount: SOURCE_AMOUNT,
        source: maker_input,
        taker_address: taker.keypair.shielded_address()?,
        payer: maker_pubkey,
        tree_id: localnet.tree_id,
    }
    .build(&maker.keypair)?
    .prove(client.indexer(), localnet.tree)?
    .instruction()?;
    let make_signature = send(client.rpc(), &maker.keypair, make_ix)?;
    client
        .confirm_private_transaction_sync(make_signature)
        .map_err(|e| anyhow!("confirm make indexed: {e:?}"))?;

    // 2. Rediscover the order from the maker's side.
    let order = index_maker(
        &maker.keypair,
        &maker.registry,
        client.indexer(),
        Duration::from_secs(60),
    )?
    .pop()
    .ok_or_else(|| anyhow!("no own swap order discovered"))?;
    assert_eq!(order.taker_viewing_pubkey, taker.keypair.viewing_pubkey());
    let order_utxo = order.order_utxo;

    // 3. Cancel back to the maker.
    let order_state = wait_for_merkle_proof(
        client.indexer(),
        localnet.tree,
        order_utxo.hash(localnet.tree_id)?,
    );
    let cancel_transaction = CancelTransactionParams {
        order_utxo: order_utxo.clone(),
        order_leaf_index: order_state.leaf_index,
        payer: maker_pubkey,
        tree_id: localnet.tree_id,
    }
    .build(&maker.keypair)?;
    let source_output_hash = SppProofOutputUtxo {
        asset: order_utxo.source_mint,
        amount: order_utxo.source_amount,
        blinding: cancel_transaction.source_output_blinding,
        owner_address: Some(maker_address),
        ..Default::default()
    }
    .hash(localnet.tree_id)
    .map_err(|e| anyhow!("source output hash: {e:?}"))?;
    let cancel_ix = cancel_transaction
        .prove(client.indexer(), localnet.tree)?
        .instruction()?;
    let cancel_signature = send(client.rpc(), &maker.keypair, cancel_ix)?;
    client
        .confirm_private_transaction_sync(cancel_signature)
        .map_err(|e| anyhow!("confirm cancel indexed: {e:?}"))?;

    // 4. The refund is indexed.
    client
        .indexer()
        .get_merkle_proofs(localnet.tree, vec![source_output_hash], None)
        .map_err(|e| anyhow!("cancel output index: {e}"))?;
    Ok(())
}

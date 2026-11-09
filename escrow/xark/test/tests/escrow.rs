mod shared;

use anyhow::{anyhow, Result};
use shared::{send, setup, Actor, TestEnv, LOCK_AMOUNT, SHIELD_AMOUNT, UNLOCK_TIMESTAMP};
use solana_signer::Signer;
use spr_client::Rpc;
use spr_test_utils::{
    test_validator_asserts::wait_for_merkle_proof,
    wallet::{sync_wallet, Wallet},
};
use spr_transaction::{AssetRegistry, Mint, SppProofOutputUtxo, SOL_ASSET_ID, SOL_MINT};
use timelock_escrow_program::error::TimelockEscrowError;
use timelock_escrow_sdk::{
    instructions::{escrow::EscrowTransactionParams, withdraw::WithdrawTransactionParams},
    state::EscrowTerms,
};
use timelock_escrow_test::assert_escrow_error;

// Timelock escrow lock-then-withdraw on the shielded pool, driven against a
// real localnet (validator + Photon indexer + prover) that `setup()` starts.
//
// Flow:
//   1. Fund (in setup): 0.5 SOL is shielded to the escrow authority.
//   2. Escrow: the program spends that 0.5 SOL UTXO -> change 0.2 SOL (back to
//      the creator) + escrow UTXO 0.3 SOL (owned by the escrow-authority PDA,
//      committed unlock timestamp already in the past). Xark escrow proof,
//      v1 transaction.
//   3. Withdraw: the creator (still holding the escrow UTXO opening locally --
//      no discovery needed) spends the escrow UTXO -> source output 0.3 SOL
//      back to itself. Xark withdraw proof, v1 transaction. Another signer
//      and a missing escrow authority are refused first.
//   4. Assert the creator's synced confidential balance after each step, and
//      that the withdraw output is indexed.
//
// Net: creator 0.5 SOL -> 0.2 SOL change + 0.3 SOL returned via withdraw.
#[test]
fn escrow_then_withdraw() -> Result<()> {
    let TestEnv {
        localnet,
        creator,
        creator_input,
    } = setup(4)?;
    let client = &localnet.client;
    let creator_address = creator.address()?;
    let creator_pubkey = creator.solana.pubkey();
    let mut wallet = Wallet::new(creator_address, AssetRegistry::default())
        .map_err(|e| anyhow!("creator wallet: {e:?}"))?;

    // escrow: lock LOCK_AMOUNT -- spend the shielded balance, append a change
    // output back to the creator plus the escrow UTXO (owned by the
    // escrow-authority PDA).
    let terms = EscrowTerms {
        creator: creator_address,
        unlock_timestamp: UNLOCK_TIMESTAMP,
    };
    let escrow_transaction = EscrowTransactionParams {
        terms,
        amount: LOCK_AMOUNT,
        source: creator_input,
        payer: creator_pubkey,
        tree_id: localnet.tree_id,
    }
    .build(&creator.shielded)?;
    let escrow_utxo = escrow_transaction.escrow_utxo.clone();
    let change_blinding = escrow_transaction.change_blinding;
    let escrow_ix = escrow_transaction
        .prove(client.indexer(), localnet.tree)?
        .instruction()?;

    let signature = send(client, &[&creator.solana], escrow_ix)?;
    client
        .confirm_private_transaction_sync(signature)
        .map_err(|e| anyhow!("confirm escrow indexed: {e:?}"))?;

    // Assert the creator's confidential balance is the change amount: the
    // 0.5 SOL note belonged to the escrow authority, and the escrow UTXO is
    // owned by the escrow-authority PDA, so neither surfaces in the creator's
    // own wallet sync.
    sync_wallet(&mut wallet, &creator.shielded, client.indexer())
        .map_err(|e| anyhow!("sync creator after escrow: {e:?}"))?;
    let expected_change = (SHIELD_AMOUNT - LOCK_AMOUNT, change_blinding);
    assert_eq!(
        balance(&wallet)?,
        (
            SOL_ASSET_ID,
            SOL_MINT,
            SHIELD_AMOUNT - LOCK_AMOUNT,
            vec![expected_change]
        )
    );

    // withdraw: after `unlock_timestamp`, spend the escrow UTXO back to the
    // creator.
    let escrow_state = wait_for_merkle_proof(
        client.indexer(),
        localnet.tree,
        escrow_utxo.hash(localnet.tree_id)?,
    );
    let withdraw_transaction = WithdrawTransactionParams {
        escrow_utxo,
        tree_id: localnet.tree_id,
        leaf_index: escrow_state.leaf_index,
        payer: creator_pubkey,
    }
    .build(&creator.shielded)?;
    let source_output_blinding = withdraw_transaction.source_output_blinding;
    let withdraw_ix = withdraw_transaction
        .prove(client.indexer(), localnet.tree)?
        .instruction()?;

    // Only the creator's signature verifies against the withdraw proof.
    let other = Actor::new(1)?;
    let mut forged_ix = withdraw_ix.clone();
    forged_ix
        .accounts
        .get_mut(1)
        .ok_or_else(|| anyhow!("withdraw has no creator account"))?
        .pubkey = other.solana.pubkey();
    assert_escrow_error(
        "a withdraw signed by another key",
        send(client, &[&creator.solana, &other.solana], forged_ix),
        TimelockEscrowError::ProofVerificationFailed,
    );
    // The program signs only for the escrow-authority PDA among the
    // forwarded accounts.
    let mut no_authority_ix = withdraw_ix.clone();
    no_authority_ix.accounts.pop();
    assert_escrow_error(
        "a withdraw without the escrow_authority account",
        send(client, &[&creator.solana], no_authority_ix),
        TimelockEscrowError::MissingEscrowAuthority,
    );

    let signature = send(client, &[&creator.solana], withdraw_ix)?;
    client
        .confirm_private_transaction_sync(signature)
        .map_err(|e| anyhow!("confirm withdraw indexed: {e:?}"))?;

    // Assert the withdrawn output landed in the creator's confidential
    // balance: the change note plus the returned escrow amount sum back to
    // the original shielded deposit, as distinct notes.
    sync_wallet(&mut wallet, &creator.shielded, client.indexer())
        .map_err(|e| anyhow!("sync creator after withdraw: {e:?}"))?;
    assert_eq!(
        balance(&wallet)?,
        (
            SOL_ASSET_ID,
            SOL_MINT,
            SHIELD_AMOUNT,
            vec![expected_change, (LOCK_AMOUNT, source_output_blinding)]
        )
    );

    let source_output_hash = SppProofOutputUtxo {
        asset: Mint::SOL,
        amount: LOCK_AMOUNT,
        blinding: source_output_blinding,
        owner_address: Some(creator_address),
        ..Default::default()
    }
    .hash(localnet.tree_id)
    .map_err(|e| anyhow!("source output hash: {e:?}"))?;
    client
        .indexer()
        .get_merkle_proofs(localnet.tree, vec![source_output_hash], None)
        .map_err(|e| anyhow!("withdraw output index: {e}"))?;
    Ok(())
}

/// The creator's SOL balance: asset id, mint, total, and each note's amount
/// and blinding in sync order.
type Balance = (u64, solana_address::Address, u64, Vec<(u64, [u8; 32])>);

fn balance(wallet: &Wallet) -> Result<Balance> {
    let balance = wallet
        .balance(SOL_MINT, None)
        .map_err(|e| anyhow!("creator balance: {e:?}"))?;
    Ok((
        balance.asset_id,
        balance.mint,
        balance.amount,
        balance
            .utxos
            .into_iter()
            .map(|note| (note.utxo.amount, note.utxo.blinding))
            .collect(),
    ))
}

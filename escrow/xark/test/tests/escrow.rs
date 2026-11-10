mod shared;

use anyhow::{anyhow, Result};
use shared::{
    creator_transactions, send, setup, Actor, TestEnv, LOCK_AMOUNT, SHIELD_AMOUNT, UNLOCK_TIMESTAMP,
};
use solana_signer::Signer;
use spr_program::InputRecords;
use spr_test_utils::wallet::{sync_wallet, Wallet};
use spr_transaction::{AssetRegistry, SOL_ASSET_ID, SOL_MINT};
use timelock_escrow_program::error::TimelockEscrowError;
use timelock_escrow_sdk::{
    instructions::{escrow::EscrowTransactionParams, withdraw::WithdrawTransactionParams},
    state::{open_escrows, EscrowTerms, EscrowUtxo},
};
use timelock_escrow_test::assert_escrow_error;

/// The escrow instruction data's viewing pubkey: after the tag and the
/// 128-byte proof.
const ESCROW_VIEWING_PUBKEY: core::ops::Range<usize> = 129..162;

// Timelock escrow lock-then-withdraw on the shielded pool, driven against a
// real localnet (validator + Photon indexer + prover) that `setup()` starts.
//
// Flow:
//   1. Fund (in setup): 0.5 SOL is shielded to the creator.
//   2. Escrow: the creator spends that 0.5 SOL UTXO -> change 0.2 SOL back to
//      itself + escrow UTXO 0.3 SOL (owned by the creator's escrow-authority
//      PDA, seeds [b"escrow_authority", creator], its terms as encrypted
//      data, committed unlock timestamp already in the past). Xark escrow
//      proof, v1 transaction. An escrow to another viewing pubkey than the
//      proof's is refused first.
//   3. Withdraw: the creator finds its open escrow by decrypting the outputs
//      of its escrow-authority PDA, which decode to the terms, and spends the
//      escrow UTXO -> source output 0.3 SOL back to itself. Xark withdraw
//      proof, v1 transaction. Another signer and a missing escrow authority
//      are refused first.
//   4. Assert the creator's synced confidential balance after each step, and
//      that no open escrow remains.
//
// Net: creator 0.5 SOL -> 0.2 SOL change + 0.3 SOL returned via withdraw.
#[test]
fn escrow_then_withdraw() -> Result<()> {
    let TestEnv {
        localnet,
        creator,
        creator_input,
        mut wallet,
    } = setup(4)?;
    let client = &localnet.client;
    let creator_address = creator.address()?;
    let creator_pubkey = creator.solana.pubkey();
    let assets = AssetRegistry::default();

    // escrow: lock LOCK_AMOUNT -- spend the creator's shielded balance, append
    // a change output back to the creator plus the escrow UTXO (owned by the
    // escrow-authority PDA).
    let escrow_transaction = EscrowTransactionParams {
        unlock_timestamp: UNLOCK_TIMESTAMP,
        amount: LOCK_AMOUNT,
        source: creator_input,
        payer: creator_pubkey,
    }
    .build(&creator.shielded)?;
    let escrow_output = escrow_transaction.escrow_output.clone();
    let change_blinding = escrow_transaction.change_blinding;
    let escrow_ix = escrow_transaction
        .prove(client.indexer(), localnet.tree, &creator.shielded)?
        .instruction()?;
    let other = Actor::new(1)?;

    // The program derives the escrow UTXO's address from the creator signer
    // and the instruction's viewing pubkey: another viewing pubkey changes
    // the escrow proof's escrow_owner public input.
    let mut other_viewer_ix = escrow_ix.clone();
    let viewing_pubkey = other_viewer_ix
        .data
        .get_mut(ESCROW_VIEWING_PUBKEY)
        .ok_or_else(|| anyhow!("escrow data has no viewing pubkey"))?;
    viewing_pubkey.copy_from_slice(other.address()?.viewing_pubkey.as_bytes());
    assert_escrow_error(
        "an escrow to another viewing pubkey",
        send(client, &[&creator.solana], other_viewer_ix),
        TimelockEscrowError::ProofVerificationFailed,
    );

    let signature = send(client, &[&creator.solana], escrow_ix)?;
    client
        .confirm_private_transaction_sync(signature)
        .map_err(|e| anyhow!("confirm escrow indexed: {e:?}"))?;

    // Assert the creator's confidential balance is the change amount: the
    // 0.5 SOL source is spent, and the escrow UTXO is owned by the
    // escrow-authority PDA, so it does not surface in the creator's own
    // wallet sync.
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

    // Find the open escrow: the escrow-authority output the creator
    // decrypts, whose data decodes to the terms the escrow circuit set.
    let [escrow]: [EscrowUtxo; 1] = open_escrows(
        &creator.shielded,
        &creator_transactions(&localnet, &creator)?,
        &assets,
    )?
    .try_into()
    .map_err(|escrows: Vec<EscrowUtxo>| {
        anyhow!("expected one open escrow, found {}", escrows.len())
    })?;
    let creator_owner = *InputRecords::new().address(&creator_address)?.owner();
    assert_eq!(
        (escrow.data, escrow.utxo.utxo_hash, escrow.utxo.utxo.amount),
        (
            EscrowTerms {
                creator: creator_owner,
                unlock: UNLOCK_TIMESTAMP,
            },
            escrow_output
                .hash(localnet.tree_id)
                .map_err(|e| anyhow!("escrow output hash: {e:?}"))?,
            LOCK_AMOUNT
        )
    );

    // withdraw: after the unlock timestamp, spend the escrow UTXO back to the
    // creator.
    let withdraw_transaction = WithdrawTransactionParams {
        escrow,
        payer: creator_pubkey,
    }
    .build(&creator.shielded)?;
    let source_output_blinding = withdraw_transaction.source_output_blinding;
    let withdraw_ix = withdraw_transaction
        .prove(client.indexer(), localnet.tree)?
        .instruction()?;

    // Only the creator's signature verifies against the withdraw proof.
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
    // the original shielded deposit, as distinct notes. No open escrow
    // remains.
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
    assert_eq!(
        open_escrows(
            &creator.shielded,
            &creator_transactions(&localnet, &creator)?,
            &assets,
        )?,
        Vec::new()
    );
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

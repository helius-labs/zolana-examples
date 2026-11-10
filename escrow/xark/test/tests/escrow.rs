mod shared;

use anyhow::{anyhow, Result};
use shared::{
    creator_transactions, send, setup, Actor, TestEnv, LOCK_AMOUNT, SHIELD_AMOUNT, UNLOCK_TIMESTAMP,
};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_signer::Signer;
use spr_program::InputRecords;
use spr_program_test::fixture;
use spr_test_utils::wallet::{sync_wallet, Wallet};
use spr_transaction::{AssetRegistry, SOL_ASSET_ID, SOL_MINT};
use timelock_escrow_program::error::TimelockEscrowError;
use timelock_escrow_sdk::{
    escrow_authority_bump, escrow_authority_seeds,
    instructions::{escrow::EscrowTransactionParams, withdraw::WithdrawTransactionParams},
    state::{open_escrows, EscrowTerms, EscrowUtxo},
};
use timelock_escrow_test::assert_escrow_error;

/// The escrow instruction data's viewing pubkey: after the tag and the
/// 128-byte proof.
const ESCROW_VIEWING_PUBKEY: core::ops::Range<usize> = 129..162;

/// Offset of the escrow instruction data's `authority_bump`: after the
/// viewing pubkey.
const ESCROW_AUTHORITY_BUMP: usize = ESCROW_VIEWING_PUBKEY.end;

/// Offset of the withdraw instruction data's `authority_bump`: after the tag,
/// the 128-byte proof and the 8-byte unlock timestamp.
const WITHDRAW_AUTHORITY_BUMP: usize = 137;

/// The highest bump below the canonical one that still derives a program
/// address of `creator`'s escrow authority.
fn non_canonical_bump(creator: &Address) -> Result<u8> {
    let [seed, creator_seed] = escrow_authority_seeds(creator);
    (0..escrow_authority_bump(creator))
        .rev()
        .find(|bump| {
            Address::create_program_address(
                &[seed, creator_seed, &[*bump]],
                &timelock_escrow_program::ID,
            )
            .is_ok()
        })
        .ok_or_else(|| anyhow!("no non-canonical escrow-authority bump"))
}

/// `ix` with its `authority_bump` at `offset` set to `bump`, after checking
/// it held `expected`.
fn with_bump(ix: &Instruction, offset: usize, expected: u8, bump: u8) -> Result<Instruction> {
    let mut ix = ix.clone();
    let authority_bump = ix
        .data
        .get_mut(offset)
        .ok_or_else(|| anyhow!("instruction data has no authority bump"))?;
    assert_eq!(*authority_bump, expected);
    *authority_bump = bump;
    Ok(ix)
}

// Timelock escrow lock-then-withdraw on the shielded pool, driven against a
// real localnet (validator + Photon indexer + prover) that `setup()` starts.
//
// Flow:
//   1. Fund (in setup): 0.5 SOL is shielded to the creator.
//   2. Escrow: the creator spends that 0.5 SOL UTXO -> change 0.2 SOL back to
//      itself + escrow UTXO 0.3 SOL (owned by the creator's escrow-authority
//      PDA, seeds [b"escrow_authority", creator], its terms as encrypted
//      data, committed unlock timestamp already in the past). Xark escrow
//      proof, v1 transaction, paid by a separate fee payer while the creator
//      signs for its source. An escrow to another viewing pubkey than the
//      proof's, one naming the fee payer as creator, and one with a
//      non-canonical authority bump are refused first.
//   3. Withdraw: the creator finds its open escrow by decrypting the outputs
//      of its escrow-authority PDA, which decode to the terms, and spends the
//      escrow UTXO -> source output 0.3 SOL back to itself. Xark withdraw
//      proof, v1 transaction. Another signer, a missing escrow authority and
//      a non-canonical authority bump are refused first.
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
    // The escrow's fee payer, a key other than the creator's.
    let fee_payer = fixture::payer();

    // escrow: lock LOCK_AMOUNT -- spend the creator's shielded balance, append
    // a change output back to the creator plus the escrow UTXO (owned by the
    // escrow-authority PDA). The fee payer pays; the creator signs for its
    // source.
    let escrow_transaction = EscrowTransactionParams {
        unlock_timestamp: UNLOCK_TIMESTAMP,
        amount: LOCK_AMOUNT,
        source: creator_input,
        payer: fee_payer.pubkey(),
    }
    .build(&creator.shielded)?;
    let escrow_output = escrow_transaction.escrow_output.clone();
    let change_blinding = escrow_transaction.change_blinding;
    let escrow_ix = escrow_transaction
        .prove(client.indexer(), localnet.tree, &creator.shielded)?
        .instruction()?;
    assert_eq!(
        escrow_ix.accounts.first().map(|meta| meta.pubkey),
        Some(creator_pubkey)
    );
    assert_ne!(fee_payer.pubkey(), creator_pubkey);
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
        send(client, &[&fee_payer, &creator.solana], other_viewer_ix),
        TimelockEscrowError::ProofVerificationFailed,
    );

    // The fee payer in the creator slot (account 0), with its own canonical
    // bump: the program derives the owner identity and the escrow authority
    // from the fee payer, which owns neither the source nor the proof's
    // escrow address. The creator still signs as the source's owner signer.
    let mut fee_payer_creator_ix = with_bump(
        &escrow_ix,
        ESCROW_AUTHORITY_BUMP,
        escrow_authority_bump(&creator_pubkey),
        escrow_authority_bump(&fee_payer.pubkey()),
    )?;
    let creator_meta = fee_payer_creator_ix
        .accounts
        .first_mut()
        .ok_or_else(|| anyhow!("escrow has no creator account"))?;
    creator_meta.pubkey = fee_payer.pubkey();
    assert!(creator_meta.is_signer);
    assert_escrow_error(
        "an escrow naming the fee payer as creator",
        send(client, &[&fee_payer, &creator.solana], fee_payer_creator_ix),
        TimelockEscrowError::ProofVerificationFailed,
    );

    // A non-canonical bump of the creator derives another escrow authority,
    // whose address is not the escrow proof's escrow_owner.
    let creator_bump = escrow_authority_bump(&creator_pubkey);
    assert_escrow_error(
        "an escrow with a non-canonical authority bump",
        send(
            client,
            &[&fee_payer, &creator.solana],
            with_bump(
                &escrow_ix,
                ESCROW_AUTHORITY_BUMP,
                creator_bump,
                non_canonical_bump(&creator_pubkey)?,
            )?,
        ),
        TimelockEscrowError::ProofVerificationFailed,
    );

    let signature = send(client, &[&fee_payer, &creator.solana], escrow_ix)?;
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

    // A non-canonical bump derives another escrow authority, which is not
    // among the forwarded accounts.
    assert_escrow_error(
        "a withdraw with a non-canonical authority bump",
        send(
            client,
            &[&creator.solana],
            with_bump(
                &withdraw_ix,
                WITHDRAW_AUTHORITY_BUMP,
                creator_bump,
                non_canonical_bump(&creator_pubkey)?,
            )?,
        ),
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

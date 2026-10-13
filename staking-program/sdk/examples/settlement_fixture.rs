//! Regenerate the Go settlement circuit witnesses with
//! `cargo run -p private-staking-sdk --example settlement_fixture`.
//!
//! Writes `prover/circuits/testdata/settlement-{4,16,cancelled}.json`: the
//! exact witness map (sorted keys) plus `Meta_EntriesHash`, which the Go
//! negative tests use to recompute the public input after mutating a context
//! field. Every key and blinding derives from fixed seeds, and both private
//! transaction hashes are computed locally, so no SPP proof is needed.
//! Cancelled entries listed as absorbed get an inclusion proof in a nullifier
//! tree holding exactly their nullifiers; the rest stay queued.
use std::collections::BTreeMap;

use anyhow::{anyhow, ensure, Result};
use num_bigint::BigUint;
use p256::{elliptic_curve::sec1::ToEncodedPoint, SecretKey};
use private_staking_program::{
    math::PoolSnapshot,
    pda,
    settlement::{self},
    state::{self, Config, IntentUtxoData},
};
use private_staking_sdk::{
    allocation::{allocate, IntentAmount},
    note::{crank_address, recipient_address, Note},
    settlement::{CancelledProof, Escrow, SettlementBuilder, SettlementParams},
};
use solana_address::Address;
use spl_stake_pool::state::Fee;
use zolana_client::NULLIFIER_TREE_HEIGHT;
use zolana_hasher::{primitives::right_align, Poseidon};
use zolana_keypair::{NullifierKey, PublicKey, ShieldedKeypair, SigningKey};
use zolana_merkle_tree::indexed::IndexedMerkleTree;
use zolana_transaction::{instructions::transact::BN254_MODULUS_DEC, Mint};

pub const TREE_ID: u16 = 7;
pub const EPOCH_ID: u64 = 43;
pub const POOL_ASSET_ID: u64 = 2;
pub const DEADLINE: i64 = 10_000;
pub const NULLIFIER_ROOT_INDEX: u16 = 5;

pub struct Fixture {
    pub builder: SettlementBuilder,
    pub config_key: Address,
    pub config: Config,
    pub epoch: Address,
    pub pool: PoolSnapshot,
    pub payer: Address,
    pub crank: ShieldedKeypair,
    pub recipients: Vec<ShieldedKeypair>,
    /// Each escrow's cancelled proof, in chain order.
    pub cancelled: Vec<Option<CancelledProof>>,
}

pub fn keypair(seed: u8) -> Result<ShieldedKeypair> {
    Ok(ShieldedKeypair::from_keypair(
        SigningKey::from_ed25519_bytes(&[seed; 32]),
    )?)
}

pub fn pool() -> PoolSnapshot {
    PoolSnapshot {
        total_lamports: 1_000_000,
        pool_token_supply: 1_000_000,
        deposit_fee: Fee {
            numerator: 1,
            denominator: 100,
        },
        withdrawal_fee: Fee {
            numerator: 1,
            denominator: 100,
        },
    }
}

pub fn config(crank: &ShieldedKeypair) -> Result<(Address, Config)> {
    let stake_pool = Address::new_from_array([12; 32]);
    let config = Config {
        discriminator: state::CONFIG_DISCRIMINATOR,
        authority: Address::new_from_array([11; 32]),
        stake_pool_program_id: spl_stake_pool::id(),
        stake_pool,
        pool_mint: Address::new_from_array([14; 32]),
        token_program_id: Address::new_from_array([15; 32]),
        escrow_tree: zolana_interface::pda::tree(TREE_ID),
        crank: crank_address(&crank.shielded_address()?)?,
        intent_fee_lamports: 50_000,
        settlement_window_seconds: 600,
        min_deposit_lamports: 10,
        min_withdrawal_pool_tokens: 10,
    };
    Ok((pda::config(&stake_pool).0, config))
}

/// An empty SPP nullifier tree, as the shielded pool initializes it.
pub fn fresh_nullifier_tree() -> Result<IndexedMerkleTree<Poseidon, usize>> {
    let modulus = BigUint::parse_bytes(BN254_MODULUS_DEC.as_bytes(), 10)
        .ok_or_else(|| anyhow!("invalid modulus"))?
        - 1u32;
    IndexedMerkleTree::<Poseidon, usize>::new_with_next_value(NULLIFIER_TREE_HEIGHT, 0, modulus)
        .map_err(|error| anyhow!("nullifier tree: {error:?}"))
}

fn field_bytes(value: &BigUint) -> Result<[u8; 32]> {
    let bytes = value.to_bytes_be();
    let mut out = [0; 32];
    let offset = out
        .len()
        .checked_sub(bytes.len())
        .ok_or_else(|| anyhow!("value exceeds 32 bytes"))?;
    out.get_mut(offset..)
        .ok_or_else(|| anyhow!("value exceeds 32 bytes"))?
        .copy_from_slice(&bytes);
    Ok(out)
}

/// Inclusion proof of `nullifier`, already appended to `tree`.
pub fn in_tree_proof(
    tree: &IndexedMerkleTree<Poseidon, usize>,
    nullifier: &[u8; 32],
) -> Result<CancelledProof> {
    let array = &tree.indexed_array;
    let element = array
        .find_element(&BigUint::from_bytes_be(nullifier))
        .ok_or_else(|| anyhow!("nullifier is not in the tree"))?;
    let next_value = if element.next_index == 0 {
        array.highest_value.clone()
    } else {
        array
            .get(element.next_index)
            .ok_or_else(|| anyhow!("missing next element"))?
            .value
            .clone()
    };
    Ok(CancelledProof::InTree {
        next_value: field_bytes(&next_value)?,
        path: tree
            .get_proof_of_leaf(element.index, true)
            .map_err(|error| anyhow!("nullifier path: {error:?}"))?,
        leaf_index: u64::try_from(element.index)?,
    })
}

/// A chain of escrows for `intents`; cancelled entries alternate between
/// SOL and pool token escrows. Cancelled entries whose index is in `absorbed`
/// are proven in the nullifier tree; the others are queued.
pub fn fixture(intents: &[IntentAmount], absorbed: &[usize]) -> Result<Fixture> {
    fixture_with(intents, absorbed, |_| {})
}

/// [`fixture`] with `edit` applied to the escrows before building.
pub fn fixture_with(
    intents: &[IntentAmount],
    absorbed: &[usize],
    edit: impl FnOnce(&mut [Escrow]),
) -> Result<Fixture> {
    let crank = keypair(200)?;
    let (config_key, config) = config(&crank)?;
    let epoch = pda::epoch(&config_key, EPOCH_ID, 0).0;
    let authority = pda::escrow_authority(&config_key).0;
    let zero_pk = NullifierKey::from_secret([0; 31]).pubkey()?;
    let pool_mint = Mint::new(config.pool_mint, POOL_ASSET_ID);
    let mut recipients = Vec::with_capacity(intents.len());
    let mut escrows = Vec::with_capacity(intents.len());
    for ((index, intent), seed) in intents.iter().enumerate().zip(20u8..) {
        let recipient = keypair(seed)?;
        let (asset, amount) = match *intent {
            IntentAmount::Deposit(amount) => (Mint::SOL, amount),
            IntentAmount::Withdrawal(amount) => (pool_mint, amount),
            IntentAmount::Cancelled if index % 2 == 0 => (Mint::SOL, 1_000 + index as u64),
            IntentAmount::Cancelled => (pool_mint, 2_000 + index as u64),
        };
        escrows.push(Escrow {
            note: Note {
                version: 1,
                tree_id: TREE_ID,
                asset: asset.asset,
                asset_id: asset.asset_id,
                amount,
                blinding: right_align(&[seed, 0x5a]),
                signing_pubkey: *PublicKey::from_pda(&authority).as_bytes(),
                nullifier_pubkey: zero_pk,
                intent: Some(IntentUtxoData {
                    epoch,
                    recipient: recipient_address(&recipient.shielded_address()?),
                }),
            },
            leaf_index: index as u64,
            cancelled: (*intent == IntentAmount::Cancelled).then_some(CancelledProof::Queued),
        });
        recipients.push(recipient);
    }

    // Absorb the listed cancellations, then prove each against the final root.
    let zero_key = NullifierKey::from_secret([0; 31]);
    let mut tree = fresh_nullifier_tree()?;
    let mut nullifiers = Vec::with_capacity(absorbed.len());
    for &index in absorbed {
        let escrow = escrows
            .get(index)
            .ok_or_else(|| anyhow!("no escrow {index}"))?;
        ensure!(
            escrow.cancelled.is_some(),
            "escrow {index} is not cancelled"
        );
        let nullifier = zero_key.nullifier(&escrow.note.hash()?, &escrow.note.blinding)?;
        tree.append(&BigUint::from_bytes_be(&nullifier))
            .map_err(|error| anyhow!("append: {error:?}"))?;
        nullifiers.push((index, nullifier));
    }
    for (index, nullifier) in nullifiers {
        let proof = in_tree_proof(&tree, &nullifier)?;
        let escrow = escrows
            .get_mut(index)
            .ok_or_else(|| anyhow!("no escrow {index}"))?;
        escrow.cancelled = Some(proof);
    }

    edit(&mut escrows);
    let cancelled = escrows
        .iter()
        .map(|escrow| escrow.cancelled.clone())
        .collect();
    let pool = pool();
    let allocation =
        allocate(&pool, intents).map_err(|error| anyhow::anyhow!("allocate: {error:?}"))?;
    let payer = Address::new_from_array([16; 32]);
    let builder = SettlementBuilder::new(SettlementParams {
        config_key,
        config: &config,
        epoch,
        settlement_deadline: DEADLINE,
        hash_chain_index: 3,
        tree_id: TREE_ID,
        payer,
        rent_payer: payer,
        escrows,
        allocation: &allocation,
        pool,
        pool_asset_id: POOL_ASSET_ID,
        nullifier_root: tree.root(),
        nullifier_root_index: NULLIFIER_ROOT_INDEX,
        ephemeral: SecretKey::from_slice(&[4; 32])?,
        salt: [5; 16],
        withdrawal_blinding_seed: right_align(&[6, 6, 6]),
        payout_blinding_seed: right_align(&[8, 8, 8]),
    })?;
    Ok(Fixture {
        builder,
        config_key,
        config,
        epoch,
        pool,
        payer,
        crank,
        recipients,
        cancelled,
    })
}

/// Deposit 30, Cancelled, Withdrawal 20, Deposit 70: payouts 29, -, 20, 70.
/// The cancellation is absorbed into the nullifier tree.
pub fn four() -> Vec<IntentAmount> {
    vec![
        IntentAmount::Deposit(30),
        IntentAmount::Cancelled,
        IntentAmount::Withdrawal(20),
        IntentAmount::Deposit(70),
    ]
}

/// Sixteen present entries mixing deposits, withdrawals and cancellations.
pub fn sixteen() -> Vec<IntentAmount> {
    (0u64..16)
        .map(|index| match index % 5 {
            3 => IntentAmount::Cancelled,
            0 | 2 => IntentAmount::Withdrawal(500 + index * 91),
            _ => IntentAmount::Deposit(1_000 + index * 137),
        })
        .collect()
}

pub const FOUR_ABSORBED: &[usize] = &[1];

/// Of the cancellations at 3, 8 and 13, the first is queued and the others
/// are absorbed.
pub const SIXTEEN_ABSORBED: &[usize] = &[8, 13];

pub fn cancelled() -> Vec<IntentAmount> {
    vec![IntentAmount::Cancelled; 3]
}

/// Queued, absorbed, queued.
pub const CANCELLED_ABSORBED: &[usize] = &[1];

/// The three fixtures: file name, intents, absorbed cancellations.
pub fn all() -> [(&'static str, Vec<IntentAmount>, &'static [usize]); 3] {
    [
        ("settlement-4.json", four(), FOUR_ABSORBED),
        ("settlement-16.json", sixteen(), SIXTEEN_ABSORBED),
        ("settlement-cancelled.json", cancelled(), CANCELLED_ABSORBED),
    ]
}

/// The JSON fixture: the witness plus `Meta_EntriesHash`.
pub fn fixture_json(fixture: &Fixture) -> Result<String> {
    let inputs = fixture.builder.proof_inputs()?;
    let mut map: BTreeMap<String, Vec<String>> = inputs.witness()?.into_iter().collect();
    let entries_hash = settlement::entries_hash(
        &fixture.builder.batch_entries(),
        &fixture.builder.entry_kinds(),
        &fixture.builder.tx_viewing_pk(),
        &[5; 16],
    )
    .map_err(|error| anyhow::anyhow!("entries hash: {error:?}"))?;
    map.insert(
        "Meta_EntriesHash".into(),
        vec![zolana_gnark_ffi_prover::decimal(&entries_hash)],
    );
    Ok(serde_json::to_string_pretty(&map)? + "\n")
}

/// Uncompressed SEC1 point of a compressed key, for tests.
pub fn uncompressed(key: &[u8; 33]) -> Result<Vec<u8>> {
    Ok(p256::PublicKey::from_sec1_bytes(key)?
        .to_encoded_point(false)
        .as_bytes()
        .to_vec())
}

fn main() -> Result<()> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../prover/circuits/testdata");
    for (name, intents, absorbed) in all() {
        let path = dir.join(name);
        std::fs::write(&path, fixture_json(&fixture(&intents, absorbed)?)?)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

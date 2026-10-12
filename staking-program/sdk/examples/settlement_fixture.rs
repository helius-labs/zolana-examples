//! Regenerate the Go settlement circuit witnesses with
//! `cargo run -p private-staking-sdk --example settlement_fixture`.
//!
//! Writes `prover/circuits/testdata/settlement-{4,16,cancelled}.json`: the
//! exact witness map (sorted keys) plus `Meta_EntriesHash`, which the Go
//! negative tests use to recompute the public input after mutating a context
//! field. Every key and blinding derives from fixed seeds, and both private
//! transaction hashes are computed locally, so no SPP proof is needed.
use std::collections::BTreeMap;

use anyhow::Result;
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
    settlement::{Escrow, SettlementBuilder, SettlementParams},
};
use solana_address::Address;
use spl_stake_pool::state::Fee;
use zolana_hasher::primitives::right_align;
use zolana_keypair::{NullifierKey, PublicKey, ShieldedKeypair, SigningKey};
use zolana_transaction::Mint;

pub const TREE_ID: u16 = 7;
pub const EPOCH_ID: u64 = 43;
pub const POOL_ASSET_ID: u64 = 2;
pub const DEADLINE: i64 = 10_000;

pub struct Fixture {
    pub builder: SettlementBuilder,
    pub config_key: Address,
    pub config: Config,
    pub epoch: Address,
    pub pool: PoolSnapshot,
    pub payer: Address,
    pub crank: ShieldedKeypair,
    pub recipients: Vec<ShieldedKeypair>,
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

/// A chain of escrows for `intents`; cancelled entries alternate between
/// SOL and pool token escrows.
pub fn fixture(intents: &[IntentAmount]) -> Result<Fixture> {
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
        });
        recipients.push(recipient);
    }
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
    })
}

/// Deposit 30, Cancelled, Withdrawal 20, Deposit 70: payouts 29, -, 20, 70.
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

pub fn cancelled() -> Vec<IntentAmount> {
    vec![IntentAmount::Cancelled; 3]
}

/// The JSON fixture: the witness plus `Meta_EntriesHash`.
pub fn fixture_json(fixture: &Fixture) -> Result<String> {
    let inputs = fixture.builder.proof_inputs()?;
    let mut map: BTreeMap<String, Vec<String>> = inputs.witness()?.into_iter().collect();
    let entries_hash = settlement::entries_hash(
        &fixture.builder.batch_entries(),
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
    for (name, intents) in [
        ("settlement-4.json", four()),
        ("settlement-16.json", sixteen()),
        ("settlement-cancelled.json", cancelled()),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, fixture_json(&fixture(&intents)?)?)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

//! Regenerate the Rust/Go encryption test vectors with
//! `cargo run -p private-staking-sdk --example encryption_fixture -- refund|payout`.
use p256::{elliptic_curve::sec1::ToEncodedPoint, SecretKey};
use private_staking_program::state::RecipientAddress;
use private_staking_sdk::{
    encryption::{self, Context},
    note::{Note, PayoutNote},
};
use solana_address::Address;
use zolana_hasher::primitives::right_align;
use zolana_keypair::PublicKey;

fn main() -> anyhow::Result<()> {
    let ephemeral = SecretKey::from_slice(&[4; 32])?;
    let user = SecretKey::from_slice(&[2; 32])?;
    let crank = SecretKey::from_slice(&[3; 32])?;
    let user_compressed: [u8; 33] = user
        .public_key()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()?;
    let crank_compressed: [u8; 33] = crank
        .public_key()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()?;
    let kind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "refund".to_string());
    let tx_viewing_pk: [u8; 33] = ephemeral
        .public_key()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()?;
    let signing_pubkey = *PublicKey::from_ed25519(&[9; 32]).as_bytes();
    let nullifier_pubkey = right_align(&[3]);
    let (plaintext, context, ciphertext) = match kind.as_str() {
        "refund" => {
            let note = Note {
                version: 1,
                tree_id: 7,
                asset: Address::default(),
                asset_id: 1,
                amount: 10_000_000,
                blinding: right_align(&[19]),
                signing_pubkey,
                nullifier_pubkey,
                intent: None,
            };
            let context = Context {
                tx_viewing_pk,
                salt: [5; 16],
                output_index: 3,
                utxo_hash: note.hash()?,
            };
            let ciphertext = encryption::encrypt(
                &note,
                &ephemeral,
                &user_compressed,
                &crank_compressed,
                &context,
            )?;
            (wincode::serialize(&note)?, context, ciphertext)
        }
        "payout" => {
            let recipient = RecipientAddress {
                signing_pubkey,
                nullifier_pubkey,
                viewing_pubkey: user_compressed,
            };
            let pool_mint = Address::new_from_array([8; 32]);
            let pool_asset_id = 2;
            let payout = PayoutNote {
                version: 1,
                tree_id: 7,
                asset: 1,
                amount: 9_500_000,
                blinding: right_align(&[21]),
            };
            let context = Context {
                tx_viewing_pk,
                salt: [5; 16],
                output_index: 3,
                utxo_hash: payout.hash(&recipient, pool_mint, pool_asset_id)?,
            };
            let ciphertext = encryption::encrypt_payout(
                &payout,
                &recipient,
                pool_mint,
                pool_asset_id,
                &ephemeral,
                &crank_compressed,
                &context,
            )?;
            (wincode::serialize(&payout)?, context, ciphertext)
        }
        other => anyhow::bail!("unknown fixture kind {other}, expected refund|payout"),
    };
    let commitment =
        encryption::commitment(&ciphertext, &user_compressed, &crank_compressed, &context)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "ephemeral_secret": &ephemeral.to_bytes()[..],
            "user_point": user.public_key().to_encoded_point(false).as_bytes(),
            "crank_point": crank.public_key().to_encoded_point(false).as_bytes(),
            "tx_viewing_pk": &context.tx_viewing_pk[..],
            "salt": &context.salt[..], "output_index": context.output_index,
            "utxo_hash": &context.utxo_hash[..], "plaintext": plaintext,
            "ciphertext": ciphertext, "commitment": &commitment[..],
        }))?
    );
    Ok(())
}

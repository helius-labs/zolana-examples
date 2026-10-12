use anyhow::{ensure, Result};
use p256::{ecdh::diffie_hellman, elliptic_curve::sec1::ToEncodedPoint, PublicKey, SecretKey};
use private_staking_program::state::RecipientAddress;
use solana_address::Address;
use zeroize::Zeroizing;
use zolana_hasher::{
    primitives::{hash_bytes, right_align},
    Hasher, Poseidon,
};
use zolana_keypair::symmetric_apply;

use crate::note::{
    Note, PayoutNote, INTENT_PLAINTEXT_BYTES, PAYOUT_PLAINTEXT_BYTES, REFUND_PLAINTEXT_BYTES,
};

pub const ENCRYPTION_INFO: [u8; 10] = *b"STAKING001";
pub const REFUND_CIPHERTEXT_BYTES: usize = 1 + 2 * REFUND_PLAINTEXT_BYTES;
pub const PAYOUT_CIPHERTEXT_BYTES: usize = 1 + 2 * PAYOUT_PLAINTEXT_BYTES;
pub const INTENT_CIPHERTEXT_BYTES: usize = 1 + 2 * INTENT_PLAINTEXT_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recipient {
    User = 0,
    Crank = 1,
}

/// SPP publishes these values alongside the ciphertext and output hash.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub tx_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
    pub output_index: u32,
    pub utxo_hash: [u8; 32],
}

pub fn commitment(
    data: &[u8],
    user: &[u8; 33],
    crank: &[u8; 33],
    context: &Context,
) -> Result<[u8; 32]> {
    let ciphertext_hash = match data.len() {
        PAYOUT_CIPHERTEXT_BYTES => hash_bytes::<PAYOUT_CIPHERTEXT_BYTES>(data.try_into()?)?,
        REFUND_CIPHERTEXT_BYTES => hash_bytes::<REFUND_CIPHERTEXT_BYTES>(data.try_into()?)?,
        INTENT_CIPHERTEXT_BYTES => hash_bytes::<INTENT_CIPHERTEXT_BYTES>(data.try_into()?)?,
        _ => anyhow::bail!("invalid staking ciphertext length"),
    };
    Ok(Poseidon::hashv(&[
        &context.utxo_hash,
        &hash_bytes(user)?,
        &hash_bytes(crank)?,
        &hash_bytes(&context.tx_viewing_pk)?,
        &right_align(&context.salt),
        &right_align(&context.output_index.to_be_bytes()),
        &ciphertext_hash,
    ])?)
}

fn public_key(secret: &SecretKey) -> Result<[u8; 33]> {
    Ok(secret
        .public_key()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()?)
}

/// Bind the ECDH result to both keys, role, output, and transaction context.
/// Including the UTXO hash prevents CTR key reuse when a batch is rebuilt.
fn shared_secret(
    secret: &SecretKey,
    counterparty: &[u8; 33],
    recipient_key: &[u8; 33],
    role: Recipient,
    context: &Context,
) -> Result<Zeroizing<[u8; 32]>> {
    let point = PublicKey::from_sec1_bytes(counterparty)?;
    let dh = diffie_hellman(secret.to_nonzero_scalar(), point.as_affine());
    let shared: &[u8; 32] = dh.raw_secret_bytes().as_slice().try_into()?;
    Ok(Zeroizing::new(Poseidon::hashv(&[
        &hash_bytes(b"stake_pool_encryption")?,
        &hash_bytes(shared)?,
        &hash_bytes(&context.tx_viewing_pk)?,
        &hash_bytes(recipient_key)?,
        &right_align(&context.salt),
        &right_align(&context.output_index.to_be_bytes()),
        &right_align(&[role as u8]),
        &context.utxo_hash,
    ])?))
}

/// Encrypt one plaintext to the user and the crank: `1 || user || crank`.
fn encrypt_plaintext(
    plaintext: &[u8],
    ephemeral: &SecretKey,
    user: &[u8; 33],
    crank: &[u8; 33],
    context: &Context,
) -> Result<Vec<u8>> {
    ensure!(
        public_key(ephemeral)? == context.tx_viewing_pk,
        "ephemeral key does not match transaction"
    );
    let mut data = Vec::with_capacity(1 + 2 * plaintext.len());
    data.push(1);
    for (role, recipient_key) in [(Recipient::User, user), (Recipient::Crank, crank)] {
        let shared = shared_secret(ephemeral, recipient_key, recipient_key, role, context)?;
        let mut ciphertext = plaintext.to_vec();
        symmetric_apply(&shared, &ENCRYPTION_INFO, &mut ciphertext)?;
        data.extend(ciphertext);
    }
    Ok(data)
}

/// Decrypt the `role` half of a `1 || user || crank` ciphertext.
fn decrypt_plaintext(
    data: &[u8],
    len: usize,
    viewing_secret: &SecretKey,
    role: Recipient,
    context: &Context,
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(data.first() == Some(&1), "unsupported ciphertext version");
    ensure!(
        data.len() == 1 + 2 * len,
        "invalid staking ciphertext length"
    );
    let key = public_key(viewing_secret)?;
    let shared = shared_secret(viewing_secret, &context.tx_viewing_pk, &key, role, context)?;
    let start = 1 + role as usize * len;
    let mut plaintext = Zeroizing::new(
        data.get(start..start + len)
            .ok_or_else(|| anyhow::anyhow!("invalid ciphertext range"))?
            .to_vec(),
    );
    symmetric_apply(&shared, &ENCRYPTION_INFO, &mut plaintext)?;
    Ok(plaintext)
}

/// Encrypt a refund or intent note (full `Note` plaintext).
pub fn encrypt(
    note: &Note,
    ephemeral: &SecretKey,
    user: &[u8; 33],
    crank: &[u8; 33],
    context: &Context,
) -> Result<Vec<u8>> {
    note.validate()?;
    ensure!(
        note.hash()? == context.utxo_hash,
        "note does not match output hash"
    );
    if let Some(intent) = note.intent {
        ensure!(
            intent.recipient.viewing_pubkey == *user,
            "wrong intent viewing recipient"
        );
    }
    let plaintext = Zeroizing::new(wincode::serialize(note)?);
    let expected_len = if note.intent.is_some() {
        INTENT_PLAINTEXT_BYTES
    } else {
        REFUND_PLAINTEXT_BYTES
    };
    ensure!(plaintext.len() == expected_len, "invalid note length");
    encrypt_plaintext(&plaintext, ephemeral, user, crank, context)
}

/// Encrypt a compact settlement payout note to the recipient's viewing key and the crank.
pub fn encrypt_payout(
    payout: &PayoutNote,
    recipient: &RecipientAddress,
    pool_mint: Address,
    pool_asset_id: u64,
    ephemeral: &SecretKey,
    crank: &[u8; 33],
    context: &Context,
) -> Result<Vec<u8>> {
    ensure!(
        payout.hash(recipient, pool_mint, pool_asset_id)? == context.utxo_hash,
        "payout note does not match output hash"
    );
    let plaintext = Zeroizing::new(wincode::serialize(payout)?);
    ensure!(
        plaintext.len() == PAYOUT_PLAINTEXT_BYTES,
        "invalid payout note length"
    );
    encrypt_plaintext(
        &plaintext,
        ephemeral,
        &recipient.viewing_pubkey,
        crank,
        context,
    )
}

/// Decrypt a compact settlement payout note and check it against the SPP commitment.
pub fn decrypt_payout(
    data: &[u8],
    viewing_secret: &SecretKey,
    role: Recipient,
    context: &Context,
    recipient: &RecipientAddress,
    pool_mint: Address,
    pool_asset_id: u64,
) -> Result<PayoutNote> {
    let plaintext = decrypt_plaintext(data, PAYOUT_PLAINTEXT_BYTES, viewing_secret, role, context)?;
    let payout: PayoutNote = wincode::deserialize_exact(&plaintext)?;
    payout.validate()?;
    ensure!(
        payout.hash(recipient, pool_mint, pool_asset_id)? == context.utxo_hash,
        "decrypted payout does not match SPP commitment"
    );
    Ok(payout)
}

/// Decrypt a refund or intent note. Compact payouts use [`decrypt_payout`].
pub fn decrypt(
    data: &[u8],
    viewing_secret: &SecretKey,
    role: Recipient,
    context: &Context,
) -> Result<Note> {
    let len = match data.len() {
        REFUND_CIPHERTEXT_BYTES => REFUND_PLAINTEXT_BYTES,
        INTENT_CIPHERTEXT_BYTES => INTENT_PLAINTEXT_BYTES,
        PAYOUT_CIPHERTEXT_BYTES => anyhow::bail!("payout ciphertext: use decrypt_payout"),
        _ => anyhow::bail!("invalid staking ciphertext length"),
    };
    let plaintext = decrypt_plaintext(data, len, viewing_secret, role, context)?;
    let note: Note = wincode::deserialize_exact(&plaintext)?;
    note.validate()?;
    ensure!(
        note.hash()? == context.utxo_hash,
        "decrypted note does not match SPP commitment"
    );
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use private_staking_program::state::{IntentUtxoData, RecipientAddress};
    use solana_address::Address;
    use zolana_keypair::{P256Pubkey, ShieldedKeypair};

    fn secret(value: u8) -> SecretKey {
        SecretKey::from_slice(&[value; 32]).unwrap()
    }

    fn fixture(intent: bool) -> (Note, SecretKey, SecretKey, SecretKey, Context) {
        let user = secret(2);
        let crank = secret(3);
        let ephemeral = secret(4);
        let signer = ShieldedKeypair::new_p256().unwrap();
        let recipient = RecipientAddress {
            signing_pubkey: *signer.signing_pubkey().as_bytes(),
            nullifier_pubkey: signer.nullifier_key.pubkey().unwrap(),
            viewing_pubkey: public_key(&user).unwrap(),
        };
        let note = Note {
            version: 1,
            tree_id: 7,
            asset: Address::default(),
            asset_id: 1,
            amount: 10_000_000,
            blinding: right_align(&[19]),
            signing_pubkey: recipient.signing_pubkey,
            nullifier_pubkey: recipient.nullifier_pubkey,
            intent: intent.then_some(IntentUtxoData {
                epoch: Address::new_from_array([12; 32]),
                recipient,
            }),
        };
        let context = Context {
            tx_viewing_pk: public_key(&ephemeral).unwrap(),
            salt: [5; 16],
            output_index: 3,
            utxo_hash: note.hash().unwrap(),
        };
        assert_eq!(
            note.output(P256Pubkey::from_bytes(public_key(&user).unwrap()).unwrap())
                .unwrap()
                .hash(7)
                .unwrap(),
            context.utxo_hash
        );
        (note, user, crank, ephemeral, context)
    }

    #[test]
    fn user_and_crank_recover_the_complete_same_note() {
        for is_intent in [false, true] {
            let (note, user, crank, ephemeral, context) = fixture(is_intent);
            let ciphertext = encrypt(
                &note,
                &ephemeral,
                &public_key(&user).unwrap(),
                &public_key(&crank).unwrap(),
                &context,
            )
            .unwrap();
            assert_eq!(
                decrypt(&ciphertext, &user, Recipient::User, &context).unwrap(),
                note
            );
            assert_eq!(
                decrypt(&ciphertext, &crank, Recipient::Crank, &context).unwrap(),
                note
            );
            if !is_intent {
                assert!(ciphertext.len() <= private_staking_program::state::MAX_CIPHERTEXT_BYTES);
            }
        }
    }

    #[test]
    fn wrong_keys_tampered_data_and_context_are_rejected() {
        let (note, user, crank, ephemeral, context) = fixture(false);
        let mut ciphertext = encrypt(
            &note,
            &ephemeral,
            &public_key(&user).unwrap(),
            &public_key(&crank).unwrap(),
            &context,
        )
        .unwrap();
        assert!(decrypt(&ciphertext, &crank, Recipient::User, &context).is_err());
        for changed in [
            Context {
                output_index: 4,
                ..context
            },
            Context {
                salt: [6; 16],
                ..context
            },
            Context {
                utxo_hash: right_align(&[20]),
                ..context
            },
        ] {
            assert!(decrypt(&ciphertext, &user, Recipient::User, &changed).is_err());
        }
        if let Some(byte) = ciphertext.get_mut(20) {
            *byte ^= 1;
        }
        assert!(decrypt(&ciphertext, &user, Recipient::User, &context).is_err());
    }

    #[test]
    fn rebuilt_payout_has_a_different_keystream_even_with_reused_transaction_metadata() {
        let (note, user, crank, ephemeral, context) = fixture(false);
        let mut changed_note = note.clone();
        changed_note.amount += 1;
        let changed_context = Context {
            utxo_hash: changed_note.hash().unwrap(),
            ..context
        };
        let a = encrypt(
            &note,
            &ephemeral,
            &public_key(&user).unwrap(),
            &public_key(&crank).unwrap(),
            &context,
        )
        .unwrap();
        let b = encrypt(
            &changed_note,
            &ephemeral,
            &public_key(&user).unwrap(),
            &public_key(&crank).unwrap(),
            &changed_context,
        )
        .unwrap();
        assert_ne!(&a[1..20], &b[1..20]);
    }

    fn payout_fixture() -> (
        PayoutNote,
        RecipientAddress,
        Address,
        SecretKey,
        SecretKey,
        SecretKey,
        Context,
    ) {
        let user = secret(2);
        let crank = secret(3);
        let ephemeral = secret(4);
        let signer = ShieldedKeypair::new_p256().unwrap();
        let recipient = RecipientAddress {
            signing_pubkey: *signer.signing_pubkey().as_bytes(),
            nullifier_pubkey: signer.nullifier_key.pubkey().unwrap(),
            viewing_pubkey: public_key(&user).unwrap(),
        };
        let pool_mint = Address::new_from_array([8; 32]);
        let payout = PayoutNote {
            version: 1,
            tree_id: 7,
            asset: 1,
            amount: 9_500_000,
            blinding: right_align(&[21]),
        };
        let context = Context {
            tx_viewing_pk: public_key(&ephemeral).unwrap(),
            salt: [5; 16],
            output_index: 3,
            utxo_hash: payout.hash(&recipient, pool_mint, 2).unwrap(),
        };
        (
            payout, recipient, pool_mint, user, crank, ephemeral, context,
        )
    }

    #[test]
    fn user_and_crank_recover_the_same_payout() {
        let (payout, recipient, pool_mint, user, crank, ephemeral, context) = payout_fixture();
        let crank_key = public_key(&crank).unwrap();
        let ciphertext = encrypt_payout(
            &payout, &recipient, pool_mint, 2, &ephemeral, &crank_key, &context,
        )
        .unwrap();
        assert_eq!(ciphertext.len(), PAYOUT_CIPHERTEXT_BYTES);
        for (key, role) in [(&user, Recipient::User), (&crank, Recipient::Crank)] {
            assert_eq!(
                decrypt_payout(&ciphertext, key, role, &context, &recipient, pool_mint, 2).unwrap(),
                payout
            );
        }
        commitment(&ciphertext, &recipient.viewing_pubkey, &crank_key, &context).unwrap();
        assert!(decrypt(&ciphertext, &user, Recipient::User, &context).is_err());
    }

    #[test]
    fn payout_rejects_tampering_wrong_context_and_wrong_recipient() {
        let (payout, recipient, pool_mint, user, crank, ephemeral, context) = payout_fixture();
        let crank_key = public_key(&crank).unwrap();
        let wrong_context = Context {
            utxo_hash: right_align(&[20]),
            ..context
        };
        assert!(encrypt_payout(
            &payout,
            &recipient,
            pool_mint,
            2,
            &ephemeral,
            &crank_key,
            &wrong_context,
        )
        .is_err());
        let mut ciphertext = encrypt_payout(
            &payout, &recipient, pool_mint, 2, &ephemeral, &crank_key, &context,
        )
        .unwrap();
        let open = |data: &[u8], key: &SecretKey, ctx: &Context, to: &RecipientAddress| {
            decrypt_payout(data, key, Recipient::User, ctx, to, pool_mint, 2)
        };
        assert!(open(&ciphertext, &crank, &context, &recipient).is_err());
        for changed in [
            Context {
                output_index: 4,
                ..context
            },
            Context {
                salt: [6; 16],
                ..context
            },
            wrong_context,
        ] {
            assert!(open(&ciphertext, &user, &changed, &recipient).is_err());
        }
        let other = RecipientAddress {
            nullifier_pubkey: right_align(&[1]),
            ..recipient
        };
        assert!(open(&ciphertext, &user, &context, &other).is_err());
        assert!(decrypt_payout(
            &ciphertext,
            &user,
            Recipient::User,
            &context,
            &recipient,
            Address::new_from_array([9; 32]),
            2
        )
        .is_err());
        let truncated = ciphertext
            .get(..PAYOUT_CIPHERTEXT_BYTES - 1)
            .unwrap()
            .to_vec();
        assert!(open(&truncated, &user, &context, &recipient).is_err());
        if let Some(byte) = ciphertext.get_mut(20) {
            *byte ^= 1;
        }
        assert!(open(&ciphertext, &user, &context, &recipient).is_err());
    }
}

use anyhow::{ensure, Result};
use private_staking_program::state::{
    IntentUtxoData, RecipientAddress, ShieldedAddress as CrankAddress,
};
use solana_address::Address;
use wincode::{SchemaRead, SchemaWrite};
use zolana_keypair::{P256Pubkey, PublicKey, ShieldedAddress};
use zolana_transaction::{
    instructions::transact::SppProofOutputUtxo, Data, DataRecord, Mint, Utxo,
};

pub const REFUND_PLAINTEXT_BYTES: usize = 150;
pub const INTENT_PLAINTEXT_BYTES: usize = REFUND_PLAINTEXT_BYTES + 131;
pub const PAYOUT_PLAINTEXT_BYTES: usize = 44;

pub const PAYOUT_ASSET_SOL: u8 = 0;
pub const PAYOUT_ASSET_POOL: u8 = 1;

/// Fixed default-ring plaintext. Includes everything needed to recover and spend
/// the note after the staking accounts close; tree_id binds its SPP commitment.
#[derive(Clone, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct Note {
    pub version: u8,
    pub tree_id: u16,
    pub asset: Address,
    pub asset_id: u64,
    pub amount: u64,
    pub blinding: [u8; 32],
    pub signing_pubkey: [u8; 34],
    pub nullifier_pubkey: [u8; 32],
    pub intent: Option<IntentUtxoData>,
}

impl Note {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported staking note version");
        PublicKey::from_bytes(self.signing_pubkey)?;
        if let Some(intent) = &self.intent {
            PublicKey::from_bytes(intent.recipient.signing_pubkey)?;
            P256Pubkey::from_bytes(intent.recipient.viewing_pubkey)?;
            intent.hash()?;
        }
        Ok(())
    }

    pub fn data_hash(&self) -> Result<[u8; 32]> {
        Ok(self
            .intent
            .as_ref()
            .map(IntentUtxoData::hash)
            .transpose()?
            .unwrap_or_default())
    }

    pub fn utxo(&self) -> Result<Utxo> {
        self.validate()?;
        let data = match self.intent {
            Some(intent) => Data::new(vec![DataRecord::UtxoData(wincode::serialize(&intent)?)]),
            None => Data::default(),
        };
        Ok(Utxo {
            owner: PublicKey::from_bytes(self.signing_pubkey)?,
            asset: Mint::new(self.asset, self.asset_id),
            amount: self.amount,
            blinding: self.blinding,
            ring_program_id: None,
            data,
        })
    }

    pub fn hash(&self) -> Result<[u8; 32]> {
        Ok(self.utxo()?.hash(
            &self.nullifier_pubkey,
            &self.data_hash()?,
            &[0; 32],
            self.tree_id,
        )?)
    }

    pub fn output(&self, viewing_pubkey: P256Pubkey) -> Result<SppProofOutputUtxo> {
        let utxo = self.utxo()?;
        let address = ShieldedAddress {
            signing_pubkey: utxo.owner,
            nullifier_pubkey: self.nullifier_pubkey,
            viewing_pubkey,
        };
        let mut output = SppProofOutputUtxo::new(utxo.asset, utxo.amount, address)?;
        output.blinding = utxo.blinding;
        if let Some(intent) = self.intent {
            output = output.with_utxo_data(wincode::serialize(&intent)?, intent.hash()?);
        }
        Ok(output)
    }
}

/// Compact settlement payout plaintext. The recipient keys come from the intent
/// and the mint from the config, so only the per-output values are encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct PayoutNote {
    pub version: u8,
    pub tree_id: u16,
    /// 0 = SOL, 1 = pool mint.
    pub asset: u8,
    pub amount: u64,
    pub blinding: [u8; 32],
}

impl PayoutNote {
    pub fn from_note(note: &Note, pool_mint: &Address) -> Result<Self> {
        ensure!(
            note.intent.is_none(),
            "payout note must not carry intent data"
        );
        let asset = if Mint::new(note.asset, note.asset_id) == Mint::SOL {
            PAYOUT_ASSET_SOL
        } else if note.asset == *pool_mint {
            PAYOUT_ASSET_POOL
        } else {
            anyhow::bail!("payout asset must be SOL or the pool mint");
        };
        let payout = Self {
            version: note.version,
            tree_id: note.tree_id,
            asset,
            amount: note.amount,
            blinding: note.blinding,
        };
        payout.validate()?;
        Ok(payout)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported payout note version");
        ensure!(self.asset <= PAYOUT_ASSET_POOL, "invalid payout asset");
        Ok(())
    }

    pub fn note(
        &self,
        recipient: &RecipientAddress,
        pool_mint: Address,
        pool_asset_id: u64,
    ) -> Result<Note> {
        self.validate()?;
        let mint = if self.asset == PAYOUT_ASSET_SOL {
            Mint::SOL
        } else {
            Mint::new(pool_mint, pool_asset_id)
        };
        let note = Note {
            version: self.version,
            tree_id: self.tree_id,
            asset: mint.asset,
            asset_id: mint.asset_id,
            amount: self.amount,
            blinding: self.blinding,
            signing_pubkey: recipient.signing_pubkey,
            nullifier_pubkey: recipient.nullifier_pubkey,
            intent: None,
        };
        note.validate()?;
        Ok(note)
    }

    pub fn hash(
        &self,
        recipient: &RecipientAddress,
        pool_mint: Address,
        pool_asset_id: u64,
    ) -> Result<[u8; 32]> {
        self.note(recipient, pool_mint, pool_asset_id)?.hash()
    }
}

pub fn recipient_address(address: &ShieldedAddress) -> RecipientAddress {
    RecipientAddress {
        signing_pubkey: *address.signing_pubkey.as_bytes(),
        nullifier_pubkey: address.nullifier_pubkey,
        viewing_pubkey: *address.viewing_pubkey.as_bytes(),
    }
}

pub fn crank_address(address: &ShieldedAddress) -> Result<CrankAddress> {
    Ok(CrankAddress {
        signing_pubkey: address.solana_address()?,
        nullifier_pubkey: address.nullifier_pubkey,
        viewing_pubkey: *address.viewing_pubkey.as_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient() -> RecipientAddress {
        RecipientAddress {
            signing_pubkey: *PublicKey::from_ed25519(&[9; 32]).as_bytes(),
            nullifier_pubkey: [3; 32],
            viewing_pubkey: [2; 33],
        }
    }

    #[test]
    fn payout_note_serializes_to_44_little_endian_bytes() {
        let payout = PayoutNote {
            version: 1,
            tree_id: 0x0102,
            asset: 1,
            amount: 0x0807_0605_0403_0201,
            blinding: [0xAA; 32],
        };
        let bytes = wincode::serialize(&payout).unwrap();
        let mut expected = vec![1, 0x02, 0x01, 1, 1, 2, 3, 4, 5, 6, 7, 8];
        expected.extend([0xAA; 32]);
        assert_eq!(bytes, expected);
        assert_eq!(bytes.len(), PAYOUT_PLAINTEXT_BYTES);
        assert_eq!(
            wincode::deserialize_exact::<PayoutNote>(&bytes).unwrap(),
            payout
        );
    }

    #[test]
    fn payout_note_round_trips_through_note() {
        let pool_mint = Address::new_from_array([7; 32]);
        for (asset, mint) in [
            (PAYOUT_ASSET_SOL, Mint::SOL),
            (PAYOUT_ASSET_POOL, Mint::new(pool_mint, 5)),
        ] {
            let payout = PayoutNote {
                version: 1,
                tree_id: 7,
                asset,
                amount: 42,
                blinding: [4; 32],
            };
            let note = payout.note(&recipient(), pool_mint, 5).unwrap();
            assert_eq!(Mint::new(note.asset, note.asset_id), mint);
            assert_eq!(note.intent, None);
            assert_eq!(PayoutNote::from_note(&note, &pool_mint).unwrap(), payout);
            assert_eq!(
                payout.hash(&recipient(), pool_mint, 5).unwrap(),
                note.hash().unwrap()
            );
        }
    }

    #[test]
    fn invalid_payout_notes_are_rejected() {
        let pool_mint = Address::new_from_array([7; 32]);
        let valid = PayoutNote {
            version: 1,
            tree_id: 7,
            asset: 0,
            amount: 42,
            blinding: [4; 32],
        };
        assert!(PayoutNote {
            version: 2,
            ..valid
        }
        .validate()
        .is_err());
        assert!(PayoutNote { asset: 2, ..valid }.validate().is_err());
        let mut note = valid.note(&recipient(), pool_mint, 5).unwrap();
        note.asset = Address::new_from_array([8; 32]);
        note.asset_id = 6;
        assert!(PayoutNote::from_note(&note, &pool_mint).is_err());
        let mut note = valid.note(&recipient(), pool_mint, 5).unwrap();
        note.intent = Some(IntentUtxoData {
            epoch: Address::new_from_array([1; 32]),
            recipient: recipient(),
        });
        assert!(PayoutNote::from_note(&note, &pool_mint).is_err());
    }
}

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

pub const PAYOUT_PLAINTEXT_BYTES: usize = 150;
pub const INTENT_PLAINTEXT_BYTES: usize = PAYOUT_PLAINTEXT_BYTES + 131;

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

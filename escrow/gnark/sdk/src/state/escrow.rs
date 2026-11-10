use anyhow::Result;
use spr_keypair::{
    constants::BLINDING_LEN, hash::poseidon, NullifierKey, PublicKey, ShieldedAddress,
};
use spr_transaction::{
    instructions::transact::SppProofOutputUtxo,
    utxo::{Blinding, SppProofInputUtxo, Utxo},
    Data, Mint,
};
use timelock_escrow_program::instructions::shared::u64_right_align;
use timelock_escrow_prover::EscrowTermsProofInput;

use crate::err;

pub trait DataHash {
    fn data_hash(&self) -> Result<[u8; 32]>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowTerms {
    pub creator: ShieldedAddress,
    pub unlock_timestamp: u64,
}

// escrow, withdraw: the escrow terms are private inputs to each circuit; the
// escrow utxo's data hash is computed over them.
impl EscrowTerms {
    pub fn data_hash(&self) -> Result<[u8; 32]> {
        EscrowTermsProofInput::try_from(self)?.data_hash()
    }
}

// escrow, withdraw: the terms enter the circuits in this form.
impl TryFrom<&EscrowTerms> for EscrowTermsProofInput {
    type Error = anyhow::Error;

    fn try_from(terms: &EscrowTerms) -> Result<Self> {
        Ok(Self {
            owner_hash: terms.creator.owner_hash().map_err(err)?,
            unlock: terms.unlock_timestamp,
        })
    }
}

// escrow, withdraw: the proofs recompute this hash from the terms.
impl DataHash for EscrowTermsProofInput {
    fn data_hash(&self) -> Result<[u8; 32]> {
        poseidon(&[&self.owner_hash, &u64_right_align(self.unlock)]).map_err(err)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowUtxo {
    pub terms: EscrowTerms,
    pub blinding: Blinding,
    pub asset: Mint,
    pub amount: u64,
}

// escrow mints to the creator's escrow-authority PDA owner; withdraw spends
// from it.
impl EscrowUtxo {
    /// The creator's escrow-authority PDA, which owns this escrow UTXO.
    pub fn escrow_authority(&self) -> Result<solana_pubkey::Pubkey> {
        let creator = self.terms.creator.solana_address().map_err(err)?;
        Ok(crate::escrow_authority_pda(&creator))
    }

    fn pda_owner(&self) -> Result<PublicKey> {
        Ok(PublicKey::from_ed25519(self.escrow_authority()?.as_array()))
    }

    /// The escrow UTXO's owner hash: the PDA owner with the zero-secret
    /// nullifier key. The escrow program recomputes it from the creator
    /// signer and passes it to the escrow proof as a public input.
    pub fn owner_hash(&self) -> Result<[u8; 32]> {
        let nullifier_pubkey = Self::nullifier_key().pubkey().map_err(err)?;
        spr_keypair::hash::owner_hash(&self.pda_owner()?, &nullifier_pubkey).map_err(err)
    }

    /// Constant nullifier key: the escrow-authority PDA is the sole
    /// authorized spender, enforced by the timelock escrow program's
    /// `invoke_signed`, not by nullifier-key secrecy.
    fn nullifier_key() -> NullifierKey {
        NullifierKey::from_secret([0u8; BLINDING_LEN])
    }
}

// escrow: the escrow output; withdraw recomputes it to match the escrow
// input it spends.
impl EscrowUtxo {
    pub fn output_utxo(&self) -> Result<SppProofOutputUtxo> {
        let data_hash = self.terms.data_hash()?;
        let nullifier_pubkey = Self::nullifier_key().pubkey().map_err(err)?;
        let owner_address = ShieldedAddress {
            signing_pubkey: self.pda_owner()?,
            nullifier_pubkey,
            viewing_pubkey: self.terms.creator.viewing_pubkey,
        };
        Ok(SppProofOutputUtxo {
            asset: self.asset,
            amount: self.amount,
            blinding: self.blinding,
            owner_address: Some(owner_address),
            ..Default::default()
        }
        .with_utxo_data(Vec::new(), data_hash))
    }
}

// withdraw: spend the escrow utxo and pay out the source funds to the
// creator.
impl EscrowUtxo {
    /// The escrow input spend: the opening (terms + blinding) is the full
    /// spend capability; the timelock escrow program signs for the PDA via
    /// `invoke_signed`.
    /// Takes the tree because the commitment and the nullifier both fold it
    /// in; choosing it afterwards would leave the pair describing a UTXO in a
    /// different tree.
    pub fn to_input_utxo(&self, tree_id: u16, leaf_index: u64) -> Result<SppProofInputUtxo> {
        let utxo = Utxo {
            owner: self.pda_owner()?,
            asset: self.asset,
            amount: self.amount,
            blinding: self.blinding,
            ring_program_id: None,
            data: Data::default(),
        };
        let data_hash = self.terms.data_hash()?;
        let key = &Self::nullifier_key();
        let nullifier_pubkey = key.pubkey()?;
        let utxo_hash = utxo.hash(&nullifier_pubkey, &data_hash, &[0; 32], tree_id)?;
        let nullifier = key.nullifier(&utxo_hash, &utxo.blinding)?;
        Ok(SppProofInputUtxo {
            utxo,
            utxo_hash,
            nullifier,
            nullifier_pubkey,
            data_hash: Some(data_hash),
            ring_data_hash: None,
            tree_id,
            leaf_index,
            cache_slot: None,
            compact: false,
            address: false,
        })
    }

    pub fn source_output(
        &self,
        recipient: ShieldedAddress,
        blinding: Blinding,
    ) -> SppProofOutputUtxo {
        SppProofOutputUtxo {
            asset: self.asset,
            amount: self.amount,
            blinding,
            owner_address: Some(recipient),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use spr_keypair::ShieldedKeypair;
    use spr_program::program::PdaOwner;
    use spr_transaction::utxo::ProofInputUtxo;

    use super::*;

    fn escrow_utxo(creator: ShieldedAddress) -> EscrowUtxo {
        EscrowUtxo {
            terms: EscrowTerms {
                creator,
                unlock_timestamp: 42,
            },
            blinding: [7u8; 32],
            asset: Mint::default(),
            amount: 1,
        }
    }

    /// The SDK's escrow owner equals what the program computes on chain
    /// (`EscrowAuthority::owner_hash`, `PdaOwner` of the creator's PDA), and
    /// the escrow output commits it.
    #[test]
    fn escrow_owner_is_the_creators_pda_owner() {
        let creator = ShieldedKeypair::new_ed25519()
            .unwrap()
            .shielded_address()
            .unwrap();
        let escrow_utxo = escrow_utxo(creator);
        let pda = crate::escrow_authority_pda(&creator.solana_address().unwrap());
        let program_owner_hash = *PdaOwner::new(&pda).unwrap().owner_hash();
        let output = escrow_utxo.output_utxo().unwrap();
        let output_owner_hash = ProofInputUtxo::try_from((&output, 0u16))
            .unwrap()
            .owner_hash;
        assert_eq!(
            (
                escrow_utxo.escrow_authority().unwrap(),
                escrow_utxo.owner_hash().unwrap(),
                output_owner_hash
            ),
            (pda, program_owner_hash, program_owner_hash)
        );
    }

    /// Each creator gets its own escrow-authority PDA.
    #[test]
    fn escrow_authority_is_per_creator() {
        let first = escrow_utxo(
            ShieldedKeypair::new_ed25519()
                .unwrap()
                .shielded_address()
                .unwrap(),
        );
        let second = escrow_utxo(
            ShieldedKeypair::new_ed25519()
                .unwrap()
                .shielded_address()
                .unwrap(),
        );
        assert_ne!(
            first.escrow_authority().unwrap(),
            second.escrow_authority().unwrap()
        );
        assert_ne!(first.owner_hash().unwrap(), second.owner_hash().unwrap());
    }
}

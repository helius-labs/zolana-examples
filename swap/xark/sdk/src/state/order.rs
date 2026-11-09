use anyhow::Result;
use solana_address::Address;
use spr_hasher::primitives::{hash_bytes, solana_owner_identity};
use spr_keypair::{
    constants::BLINDING_LEN, hash::poseidon, CompressedShieldedAddress, NullifierKey, PublicKey,
    ShieldedAddress, X25519Pubkey,
};
use spr_transaction::{
    instructions::transact::SppProofOutputUtxo,
    utxo::{Blinding, SppProofInputUtxo, Utxo},
    Data, Mint,
};
use swap_program::circuits;
use wincode::{SchemaRead, SchemaWrite};

use crate::{err, shared::field};

/// A `u64` as a big-endian field element.
pub fn u64_right_align(value: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (byte, value_byte) in bytes.iter_mut().rev().zip(value.to_le_bytes()) {
        *byte = value_byte;
    }
    bytes
}

pub trait DataHash {
    fn data_hash(&self) -> Result<[u8; 32]>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderTerms {
    pub destination_mint: Address,
    pub destination_amount: u64,

    pub destination: ShieldedAddress,
    pub taker: Address,

    pub expiry: u64,
}

#[derive(SchemaWrite, SchemaRead, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlainTextData {
    pub destination_asset_id: u64,
    pub destination_amount: u64,
    pub expiry: u64,
    pub taker: Address,
}

/// The order UTXO's two representations: the output minted at create time and
/// the spend consumed at take/cancel time. Both share the order-authority PDA
/// as the ed25519 owner key and the zero-secret nullifier key -- the synthetic
/// shielded address that the swap program signs for via `invoke_signed` -- so
/// their utxo hashes are byte-identical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderUtxo {
    pub terms: OrderTerms,
    pub blinding: Blinding,
    pub source_mint: Mint,
    pub source_amount: u64,
    pub destination_asset_id: u64,
}

// All instructions: the order terms are private inputs to each circuit; the
// order utxo's data hash is computed over them.
impl OrderTerms {
    // The utxo itself commits source amount and mint.
    // The data hash constrains:
    // 1. the mint we want to swap into
    // 2. how many tokens of the mint we want to swap into
    // 3. which shielded pubkey the swap settlement will go to
    // 4. the order expiry
    // 5. the taker allowed to take
    pub fn data_hash(&self) -> Result<[u8; 32]> {
        let maker_address = CompressedShieldedAddress {
            owner_hash: self.destination.owner_hash().map_err(err)?,
            viewing_pubkey: self.destination.viewing_pubkey,
        }
        .hash()
        .map_err(err)?;
        poseidon(&[
            &hash_bytes(self.destination_mint.as_array()).map_err(err)?,
            &u64_right_align(self.destination_amount),
            &maker_address,
            &u64_right_align(self.expiry),
            &self.taker.data_hash()?,
        ])
        .map_err(err)
    }

    /// The terms as the circuits take them; the circuits recompute
    /// [`Self::data_hash`] from them.
    pub fn circuit_terms(&self) -> Result<circuits::OrderTerms> {
        Ok(circuits::OrderTerms {
            destination_asset: field(&hash_bytes(self.destination_mint.as_array()).map_err(err)?)?,
            destination_amount: self.destination_amount,
            maker_owner_hash: field(&self.destination.owner_hash().map_err(err)?)?,
            maker_viewing_pk: *self.destination.viewing_pubkey.as_bytes(),
            expiry: self.expiry,
            taker_pk_fe: field(&self.taker.data_hash()?)?,
        })
    }
}

// All instructions: the taker pubkey as the `taker_pk_fe` terms field. A
// taker's UTXO owner is `Poseidon(owner_proof_input_hash(pk), nullifier_pk)`,
// so this is the same algorithm-tagged Solana identity, not the bare
// `hash_bytes`.
impl DataHash for Address {
    fn data_hash(&self) -> Result<[u8; 32]> {
        solana_owner_identity(self.as_array()).map_err(err)
    }
}

// make mints to the synthetic order owner; take and cancel spend from it.
impl OrderUtxo {
    fn pda_owner() -> PublicKey {
        PublicKey::from_ed25519(crate::order_authority_pda().as_array())
    }

    /// Constant nullifier key so that both counterparties can spend this utxo.
    fn nullifier_key() -> NullifierKey {
        NullifierKey::from_secret([0u8; BLINDING_LEN])
    }
}

// make: the taker-readable note payload encrypted into the order
// output slot; discover decodes it back into the terms.
impl OrderTerms {
    pub fn to_plaintext(&self, destination_asset_id: u64) -> PlainTextData {
        PlainTextData {
            destination_asset_id,
            destination_amount: self.destination_amount,
            expiry: self.expiry,
            taker: self.taker,
        }
    }
}

// make: serialized into the order note; discover: deserialized from
// it.
impl PlainTextData {
    pub fn serialize(&self) -> Result<Vec<u8>> {
        wincode::serialize(self).map_err(err)
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        wincode::deserialize_exact(bytes).map_err(err)
    }
}

// make: the order output; discover recomputes it to match the output
// slots fetched from the indexer.
impl OrderUtxo {
    /// The taker's viewing pubkey makes the order slot ciphertext
    /// readable by the taker; its `owner_hash` is a constant across orders.
    pub fn output_utxo(&self, taker_viewing_pubkey: X25519Pubkey) -> Result<SppProofOutputUtxo> {
        let data_hash = self.terms.data_hash()?;
        let nullifier_pubkey = Self::nullifier_key().pubkey().map_err(err)?;
        let owner_address = ShieldedAddress {
            signing_pubkey: Self::pda_owner(),
            nullifier_pubkey,
            viewing_pubkey: taker_viewing_pubkey,
        };
        Ok(SppProofOutputUtxo {
            asset: self.source_mint,
            amount: self.source_amount,
            blinding: self.blinding,
            owner_address: Some(owner_address),
            ..Default::default()
        }
        .with_utxo_data(
            self.terms
                .to_plaintext(self.destination_asset_id)
                .serialize()?,
            data_hash,
        ))
    }
}

// take, cancel: spend the order utxo and pay out the
// source funds (to the taker on take, back to the maker on cancel).
impl OrderUtxo {
    /// The order input spend, committed under the tree with the raw id
    /// `tree_id`: the opening (terms + blinding) is the full spend capability;
    /// the swap program signs for the PDA via `invoke_signed`.
    ///
    /// `tree_id` is a parameter rather than something the caller sets
    /// afterwards, because both the commitment and the nullifier fold it in.
    pub fn to_input_utxo(&self, tree_id: u16, leaf_index: u64) -> Result<SppProofInputUtxo> {
        let utxo = Utxo {
            owner: Self::pda_owner(),
            asset: self.source_mint,
            amount: self.source_amount,
            blinding: self.blinding,
            ring_program_id: None,
            data: Data::default(),
        };
        let data_hash = self.terms.data_hash()?;
        let key = Self::nullifier_key();
        let nullifier_pubkey = key.pubkey()?;
        let utxo_hash = utxo.hash(&nullifier_pubkey, &data_hash, &[0; 32], tree_id)?;
        let nullifier = key.nullifier(&utxo_hash, &utxo.blinding)?;
        Ok(SppProofInputUtxo {
            address: false,
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
        })
    }

    pub fn source_output(
        &self,
        recipient: ShieldedAddress,
        blinding: Blinding,
    ) -> SppProofOutputUtxo {
        SppProofOutputUtxo {
            asset: self.source_mint,
            amount: self.source_amount,
            blinding,
            owner_address: Some(recipient),
            ..Default::default()
        }
    }
}

// take: pay out the maker's destination funds.
impl OrderUtxo {
    pub fn destination_output(
        &self,
        recipient: ShieldedAddress,
        blinding: Blinding,
    ) -> SppProofOutputUtxo {
        SppProofOutputUtxo {
            asset: Mint::new(self.terms.destination_mint, self.destination_asset_id),
            amount: self.terms.destination_amount,
            blinding,
            owner_address: Some(recipient),
            ..Default::default()
        }
    }
}

impl OrderUtxo {
    /// Reconstruct the maker payout using the order opening and the first
    /// nullifier published by its settlement. No settlement ciphertext is needed.
    pub fn derived_destination_output(
        &self,
        first_nullifier: &[u8; 32],
    ) -> Result<SppProofOutputUtxo> {
        use spr_transaction::utxo::{derive_output_blinding_seed, derive_transact_output_blinding};
        let blinding_seed = crate::instructions::take::take_blinding_seed(&self.blinding)?;
        let seed = derive_output_blinding_seed(first_nullifier, &blinding_seed).map_err(err)?;
        let blinding = derive_transact_output_blinding(first_nullifier, &seed, 1).map_err(err)?;
        Ok(self.destination_output(self.terms.destination, blinding))
    }
}

#[cfg(test)]
mod tests {
    use solana_keypair::Keypair;
    use spr_keypair::ShieldedKeypair;
    use spr_transaction::SOL_MINT;

    use super::*;

    #[test]
    fn circuit_terms_recompute_the_native_data_hash() {
        let destination = ShieldedKeypair::from_keypair(&Keypair::new_from_array([21u8; 32]))
            .expect("destination keypair")
            .shielded_address()
            .expect("destination address");
        let terms = OrderTerms {
            destination_mint: SOL_MINT,
            destination_amount: 250_000,
            destination,
            taker: Address::new_from_array([5u8; 32]),
            expiry: 1_700_000_000,
        };
        let circuit = terms.circuit_terms().expect("circuit terms");
        assert_eq!(
            spr_program::to_bytes(&circuit.data_hash().expect("circuit data hash")),
            terms.data_hash().expect("native data hash")
        );
    }
}

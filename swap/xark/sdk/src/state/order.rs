use anyhow::{bail, Result};
use spr_keypair::{PublicKey, ShieldedAddress, X25519Pubkey};
use spr_program::{circuit::DataHash, ClientInputs, EncryptionCodec, InputRecords};
use spr_transaction::{AssetRegistry, Mint, SppProofOutputUtxo, WalletUtxo};
use swap_program::circuits::{self, OrderTermsClient};

use crate::{err, order_authority, shared::field};

/// The order terms: what the maker buys, where the payout goes, until when the
/// order can be taken and by whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderTerms {
    pub destination_mint: Mint,
    pub destination_amount: u64,
    /// The maker's shielded address: receives the destination on take and the
    /// source on cancel; its Solana key signs the cancel.
    pub maker: ShieldedAddress,
    /// Unix seconds.
    pub expiry: u64,
    /// The designated taker's owner identity ([`crate::shared::identity`] of
    /// its Solana address), or `None` for an order anyone holding the opening
    /// may take.
    pub taker: Option<[u8; 32]>,
}

impl OrderTerms {
    /// The terms as the circuits take them.
    pub fn circuit_terms(&self) -> Result<OrderTermsClient> {
        Ok(OrderTermsClient {
            destination_mint: self.destination_mint,
            destination_amount: self.destination_amount,
            maker: self.maker,
            expiry: self.expiry,
            taker_pk_fe: field(self.taker.unwrap_or_default())?,
        })
    }

    /// The order UTXO's state, which the circuits hash and encode.
    pub fn state(&self) -> Result<circuits::OrderTerms> {
        circuits::OrderTerms::from_client(self.circuit_terms()?, &mut InputRecords::new())
            .map_err(err)
    }

    /// `Poseidon(DATA, Poseidon(order terms))`, the order UTXO's data hash.
    pub fn data_hash(&self) -> Result<[u8; 32]> {
        let terms_hash = DataHash::hash(&self.state()?).map_err(err)?;
        spr_program::hasher::data_hash(&spr_program::to_bytes(&terms_hash)).map_err(err)
    }

    /// The order UTXO's encrypted payload.
    pub fn payload(&self) -> Result<Vec<u8>> {
        self.state()?.serialize_for_encryption().map_err(err)
    }

    /// Decodes an order UTXO's payload, resolving the destination mint's asset
    /// id in `registry`. The maker's signing key decodes as an ed25519 key.
    pub fn from_payload(payload: &[u8], registry: &AssetRegistry) -> Result<Self> {
        let state = circuits::OrderTerms::deserialize_after_decryption(payload).map_err(err)?;
        let (_, key, nullifier_pubkey) = state.maker.owner().preimage();
        let terms = Self {
            destination_mint: resolve_mint(registry, &state.destination_mint)?,
            destination_amount: state.destination_amount,
            maker: ShieldedAddress {
                signing_pubkey: PublicKey::from_ed25519(&key),
                nullifier_pubkey,
                viewing_pubkey: X25519Pubkey::from_bytes(*state.maker.viewing_pubkey())
                    .map_err(err)?,
            },
            expiry: state.expiry,
            taker: Some(spr_program::to_bytes(&state.taker_pk_fe)).filter(|id| *id != [0; 32]),
        };
        if terms.payload()? != payload {
            bail!("the order payload does not decode to its terms");
        }
        Ok(terms)
    }
}

/// The mint of a 32-byte asset: all zeros is SOL, any other is looked up in
/// `registry`.
fn resolve_mint(registry: &AssetRegistry, asset: &[u8; 32]) -> Result<Mint> {
    if asset == Mint::SOL.asset.as_array() {
        return Ok(Mint::SOL);
    }
    registry
        .mint(&solana_address::Address::new_from_array(*asset))
        .map_err(err)
}

/// The order UTXO: `source_amount` of `source_mint` owned by the
/// order-authority PDA under the zero nullifier key, with the terms as its
/// state. Its opening (terms and blinding) is the complete spend capability;
/// the program signs for the PDA only through `take` and `cancel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderUtxo {
    pub terms: OrderTerms,
    /// Derived by the make transaction from its first nullifier, blinding
    /// seed and output slot.
    pub blinding: [u8; 32],
    pub source_mint: Mint,
    pub source_amount: u64,
}

impl OrderUtxo {
    /// The order output. The owner's viewing key does not enter the
    /// commitment; make encrypts the output to the taker's.
    pub fn output_utxo(&self, viewing_pubkey: X25519Pubkey) -> Result<SppProofOutputUtxo> {
        Ok(SppProofOutputUtxo {
            asset: self.source_mint,
            amount: self.source_amount,
            blinding: self.blinding,
            owner_address: Some(order_authority().address(viewing_pubkey).map_err(err)?),
            ..Default::default()
        }
        .with_utxo_data(self.terms.payload()?, self.terms.data_hash()?))
    }

    /// The order UTXO's commitment in tree `tree_id`.
    pub fn hash(&self, tree_id: u16) -> Result<[u8; 32]> {
        self.output_utxo(self.terms.maker.viewing_pubkey)?
            .hash(tree_id)
            .map_err(err)
    }

    /// The order UTXO as an SPP input at `leaf_index` of tree `tree_id`.
    /// Anyone holding the opening can build it; spending needs the PDA
    /// signature only the program gives.
    pub fn to_input_utxo(&self, tree_id: u16, leaf_index: u64) -> Result<WalletUtxo> {
        order_authority()
            .input(
                &self.output_utxo(self.terms.maker.viewing_pubkey)?,
                tree_id,
                leaf_index,
            )
            .map_err(err)
    }

    /// The maker's payout of the take whose first nullifier is
    /// `first_nullifier`, from the order opening alone: the take circuit fixes
    /// its blinding seed to [`crate::instructions::take::take_blinding_seed`],
    /// and the payout is output slot 1. No settlement ciphertext is needed.
    pub fn derived_destination_output(
        &self,
        first_nullifier: &[u8; 32],
    ) -> Result<SppProofOutputUtxo> {
        let blinding_seed = crate::instructions::take::take_blinding_seed(&self.blinding)?;
        let seed = spr_program::derive_output_blinding_seed(first_nullifier, &blinding_seed)
            .map_err(err)?;
        let blinding =
            spr_program::derive_transact_output_blinding(first_nullifier, &seed, 1).map_err(err)?;
        Ok(SppProofOutputUtxo {
            asset: self.terms.destination_mint,
            amount: self.terms.destination_amount,
            blinding,
            owner_address: Some(self.terms.maker),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use solana_address::Address;
    use solana_keypair::Keypair;
    use spr_keypair::ShieldedKeypair;

    use super::*;

    fn terms(taker: Option<[u8; 32]>) -> OrderTerms {
        let maker = ShieldedKeypair::from_keypair(&Keypair::new_from_array([21u8; 32]))
            .unwrap()
            .shielded_address()
            .unwrap();
        OrderTerms {
            destination_mint: Mint::new(Address::new_from_array([4; 32]), 2),
            destination_amount: 250_000,
            maker,
            expiry: 1_700_000_000,
            taker,
        }
    }

    fn registry() -> AssetRegistry {
        let mut registry = AssetRegistry::default();
        registry
            .insert(2, Address::new_from_array([4; 32]))
            .unwrap();
        registry
    }

    #[test]
    fn payload_round_trips_designated_and_open_terms() {
        // 1. Encode and decode a designated and an open order.
        for terms in [
            terms(Some([0, 5, 6, 7].repeat(8).try_into().unwrap())),
            terms(None),
        ] {
            let payload = terms.payload().unwrap();
            assert_eq!(payload.len(), circuits::PAYLOAD_LEN);
            // 2. The decoded terms equal the encoded ones.
            assert_eq!(
                OrderTerms::from_payload(&payload, &registry()).unwrap(),
                terms
            );
        }
    }

    #[test]
    fn payload_with_trailing_bytes_is_refused() {
        let mut payload = terms(None).payload().unwrap();
        payload.push(0);
        assert!(OrderTerms::from_payload(&payload, &registry()).is_err());
    }
}

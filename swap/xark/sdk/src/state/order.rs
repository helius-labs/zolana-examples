use anyhow::{ensure, Result};
use spr_keypair::{ShieldedAddress, X25519Pubkey};
use spr_program::{circuit::DataHash, ClientInputs, DataUtxo, EncryptionCodec, InputRecords};
use spr_transaction::{AssetRegistry, Mint, SppProofOutputUtxo, WalletUtxo};
use swap_program::circuits::{self, OrderTermsClient};

use crate::{err, order_authority};

/// The order terms: what the maker buys, where the payout goes, until when the
/// order can be taken and by whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderTerms {
    pub destination_mint: Mint,
    pub destination_amount: u64,
    /// The maker's shielded address: receives the destination on take and the
    /// source on cancel; its Solana key signs the cancel. The order UTXO
    /// commits its owner; the viewing key only encrypts the maker payout.
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
            maker: *InputRecords::new()
                .address(&self.maker)
                .map_err(err)?
                .owner(),
            expiry: self.expiry,
            taker_pk_fe: self.taker.unwrap_or_default(),
        })
    }

    /// The order UTXO's state, which the SDK hashes and encodes.
    pub fn state(&self) -> Result<circuits::OrderTerms> {
        circuits::OrderTerms::from_client(self.circuit_terms()?, &mut InputRecords::new())
            .map_err(err)
    }

    /// The terms of a decoded order UTXO state whose maker is `maker`, with
    /// the destination mint's asset id resolved in `registry`.
    pub fn from_state(
        state: &circuits::OrderTerms,
        maker: ShieldedAddress,
        registry: &AssetRegistry,
    ) -> Result<Self> {
        let terms = Self {
            destination_mint: resolve_mint(registry, &state.destination_mint)?,
            destination_amount: state.destination_amount,
            maker,
            expiry: state.expiry,
            taker: Some(state.taker_pk_fe).filter(|id| *id != [0; 32]),
        };
        ensure!(terms.state()? == *state, "the order names another maker");
        Ok(terms)
    }

    /// The order UTXO's encrypted `utxo_data`, the state's Borsh encoding,
    /// and its data hash, `Poseidon(DATA, state hash)`, from one state.
    fn utxo_data(&self) -> Result<(Vec<u8>, [u8; 32])> {
        let state = self.state()?;
        let terms_hash = DataHash::hash(&state).map_err(err)?;
        let data_hash =
            spr_program::hasher::data_hash(&spr_program::to_bytes(&terms_hash)).map_err(err)?;
        Ok((state.serialize_for_encryption().map_err(err)?, data_hash))
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
    /// The order of a decoded order UTXO whose maker is `maker`. The SDK's
    /// decode ([`DataUtxo::try_from`]) has checked the state against the
    /// UTXO's commitment.
    pub fn from_data_utxo(
        order: DataUtxo<circuits::OrderTerms>,
        maker: ShieldedAddress,
        registry: &AssetRegistry,
    ) -> Result<Self> {
        Ok(Self {
            terms: OrderTerms::from_state(&order.data, maker, registry)?,
            blinding: order.utxo.utxo.blinding,
            source_mint: order.utxo.utxo.asset,
            source_amount: order.utxo.utxo.amount,
        })
    }

    /// The order output. The owner's viewing key does not enter the
    /// commitment; make encrypts the output to the taker's.
    pub fn output_utxo(&self, viewing_pubkey: X25519Pubkey) -> Result<SppProofOutputUtxo> {
        let (payload, data_hash) = self.terms.utxo_data()?;
        Ok(SppProofOutputUtxo {
            asset: self.source_mint,
            amount: self.source_amount,
            blinding: self.blinding,
            owner_address: Some(order_authority().address(viewing_pubkey).map_err(err)?),
            ..Default::default()
        }
        .with_utxo_data(payload, data_hash))
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

    fn order(terms: OrderTerms) -> OrderUtxo {
        OrderUtxo {
            terms,
            blinding: crate::shared::test_blinding(7),
            source_mint: Mint::SOL,
            source_amount: 400_000,
        }
    }

    #[test]
    fn order_utxo_decodes_designated_and_open_terms() {
        // 1. Decode the order UTXO of a designated and an open order.
        for terms in [
            terms(Some([0, 5, 6, 7].repeat(8).try_into().unwrap())),
            terms(None),
        ] {
            let order = order(terms);
            let input = order.to_input_utxo(0, 3).unwrap();
            // 2. The decoded order equals the encoded one.
            assert_eq!(
                OrderUtxo::from_data_utxo(
                    DataUtxo::try_from(input).unwrap(),
                    order.terms.maker,
                    &registry()
                )
                .unwrap(),
                order
            );
        }
    }

    #[test]
    fn order_utxo_with_another_payload_is_refused() {
        let order = order(terms(None));
        let mut input = order.to_input_utxo(0, 3).unwrap();
        let mut other = order.terms.clone();
        other.destination_amount += 1;
        input.utxo.data = SppProofOutputUtxo::default()
            .with_utxo_data(other.utxo_data().unwrap().0, [0; 32])
            .data;
        assert!(DataUtxo::<circuits::OrderTerms>::try_from(input).is_err());
    }

    #[test]
    fn order_of_another_maker_is_refused() {
        let order = order(terms(None));
        let decoded = DataUtxo::try_from(order.to_input_utxo(0, 3).unwrap()).unwrap();
        let other = ShieldedKeypair::from_keypair(&Keypair::new_from_array([22u8; 32]))
            .unwrap()
            .shielded_address()
            .unwrap();
        assert!(OrderUtxo::from_data_utxo(decoded, other, &registry()).is_err());
    }
}

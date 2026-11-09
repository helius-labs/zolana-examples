use spr_program::{
    circuit::{hash_bytes, DataHash, UtxoData},
    input, poseidon, require, require_eq, Asset, CircuitError, ClientInputs, EncryptionCodec,
    Field, ShieldedAddress, UtxoOwner,
};

/// The order terms, the order UTXO's state. Its data hash is
/// `Poseidon(DATA, hash())`, and its encrypted `utxo_data` is the
/// [`EncryptionCodec`] payload, from which the taker and the maker rebuild the
/// terms. See `swap_program.md`, Order Terms.
#[input]
#[derive(ClientInputs)]
pub struct OrderTerms {
    /// The mint the maker buys.
    pub destination_mint: Asset,
    pub destination_amount: u64,
    /// The maker's shielded address: receives the destination on take and
    /// the source on cancel, and its signing key signs the cancel.
    pub maker: ShieldedAddress,
    /// Unix seconds; revealed at take and cancel and checked against the
    /// Clock by the program.
    pub expiry: u64,
    /// The designated taker's owner identity (`solana_owner_identity` of its
    /// Solana address), or 0 for an order anyone holding the opening may take.
    pub taker_pk_fe: Field,
}

impl DataHash for OrderTerms {
    /// `Poseidon(hash_bytes(destination_mint), destination_amount,
    /// maker_address, expiry, taker_pk_fe)`, where `maker_address` is
    /// `Poseidon(maker_owner_hash, hash_bytes(maker_viewing_pk))`.
    fn hash(&self) -> Result<Field, CircuitError> {
        poseidon(&[
            hash_bytes(&self.destination_mint)?,
            Field::from(self.destination_amount),
            self.maker.hash()?,
            Field::from(self.expiry),
            self.taker_pk_fe,
        ])
    }
}

impl UtxoData for OrderTerms {}

/// The state a new order UTXO starts from before `make` sets the terms.
impl Default for OrderTerms {
    fn default() -> Self {
        Self {
            destination_mint: [0; 32],
            destination_amount: 0,
            maker: ShieldedAddress::default(),
            expiry: 0,
            taker_pk_fe: Field::zero(),
        }
    }
}

impl OrderTerms {
    /// `destination_amount > 0`; the `u64` input is range-checked on entry.
    pub(super) fn assert_valid(&self) {
        require(self.destination_amount != 0);
    }

    /// An open order (`taker_pk_fe == 0`) accepts any taker; otherwise the
    /// taker input's owner must be `Poseidon(taker_pk_fe, nullifier_pk)`,
    /// with the input owner's own nullifier public key. One constraint:
    /// `taker_pk_fe * (owner_hash - Poseidon(taker_pk_fe, nullifier_pk)) == 0`.
    pub(super) fn assert_taker(&self, taker: &UtxoOwner) -> Result<(), CircuitError> {
        let designated = poseidon(&[self.taker_pk_fe, taker.nullifier_pk()])?;
        require_eq(
            self.taker_pk_fe * (taker.hash()? - designated),
            Field::zero(),
        );
        Ok(())
    }
}

/// The order UTXO's encrypted payload: destination mint (32 bytes),
/// destination amount (8, little endian), the maker's owner preimage (tag,
/// signing key, nullifier public key: 65), the maker's viewing public key
/// (33), expiry (8, little endian) and `taker_pk_fe` (32, big endian), 178
/// bytes in all.
impl EncryptionCodec for OrderTerms {
    fn serialize_for_encryption(&self) -> borsh::io::Result<Vec<u8>> {
        #[cfg(not(xark))]
        {
            let mut bytes = Vec::with_capacity(PAYLOAD_LEN);
            borsh::BorshSerialize::serialize(&self.destination_mint, &mut bytes)?;
            borsh::BorshSerialize::serialize(&self.destination_amount, &mut bytes)?;
            borsh::BorshSerialize::serialize(self.maker.owner(), &mut bytes)?;
            borsh::BorshSerialize::serialize(self.maker.viewing_pubkey(), &mut bytes)?;
            borsh::BorshSerialize::serialize(&self.expiry, &mut bytes)?;
            borsh::BorshSerialize::serialize(
                &spr_program::to_bytes(&self.taker_pk_fe),
                &mut bytes,
            )?;
            Ok(bytes)
        }
        #[cfg(xark)]
        {
            panic!("order terms serialization requires native execution")
        }
    }

    fn deserialize_after_decryption(bytes: &[u8]) -> borsh::io::Result<Self> {
        #[cfg(not(xark))]
        {
            use borsh::BorshDeserialize;
            let invalid = |error: CircuitError| {
                borsh::io::Error::new(borsh::io::ErrorKind::InvalidData, error.to_string())
            };
            let mut reader = bytes;
            let destination_mint = Asset::deserialize(&mut reader)?;
            let destination_amount = u64::deserialize(&mut reader)?;
            let owner = UtxoOwner::deserialize(&mut reader)?;
            let viewing_pubkey = <[u8; 33]>::deserialize(&mut reader)?;
            let expiry = u64::deserialize(&mut reader)?;
            let taker_pk_fe =
                spr_program::from_bytes(&<[u8; 32]>::deserialize(&mut reader)?).map_err(invalid)?;
            if !reader.is_empty() {
                return Err(borsh::io::Error::new(
                    borsh::io::ErrorKind::InvalidData,
                    "trailing bytes after the order terms",
                ));
            }
            Ok(Self {
                destination_mint,
                destination_amount,
                maker: ShieldedAddress::new(owner, viewing_pubkey),
                expiry,
                taker_pk_fe,
            })
        }
        #[cfg(xark)]
        {
            let _ = bytes;
            panic!("order terms deserialization requires native execution")
        }
    }
}

/// The length of the order UTXO's encrypted payload.
#[cfg(not(xark))]
pub const PAYLOAD_LEN: usize = 32 + 8 + 65 + 33 + 8 + 32;

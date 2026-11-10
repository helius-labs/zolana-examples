use borsh::{BorshDeserialize, BorshSerialize};
use spr_program::{
    input, poseidon, require, require_eq, Asset, CircuitError, ClientInputs, Field, UtxoOwner,
    UtxoState,
};

/// The order terms, the order UTXO's state (`DataUtxo<OrderTerms>`). The SDK
/// hashes and encodes them: the order UTXO's data hash is
/// `Poseidon(DATA, Poseidon(discriminator, hash_bytes(destination_mint),
/// destination_amount, maker_owner_hash, expiry, hash_bytes(taker_pk_fe)))`
/// and its encrypted `utxo_data` is the terms' Borsh encoding. See
/// `swap_program.md`, Order Terms.
#[input]
#[derive(Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, UtxoState, ClientInputs)]
pub struct OrderTerms {
    /// The mint the maker buys.
    pub destination_mint: Asset,
    pub destination_amount: u64,
    /// The maker, the owner of the source make spends: receives the
    /// destination on take and the source on cancel, and its signing key
    /// signs the cancel.
    pub maker: UtxoOwner,
    /// Unix seconds; revealed at take and cancel and checked against the
    /// Clock by the program.
    pub expiry: u64,
    /// The designated taker's owner identity (`solana_owner_identity` of its
    /// Solana address) as 32 big-endian bytes, or zero for an order anyone
    /// holding the opening may take.
    pub taker_pk_fe: [u8; 32],
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
        let taker_pk_fe = self.taker_pk_fe_field();
        let designated = poseidon(&[taker_pk_fe, taker.nullifier_pk()])?;
        require_eq(taker_pk_fe * (taker.hash()? - designated), Field::zero());
        Ok(())
    }

    /// `taker_pk_fe` as a field element: its bytes packed big-endian, a
    /// linear combination with no constraint of its own.
    fn taker_pk_fe_field(&self) -> Field {
        let mut packed = Field::zero();
        for byte in self.taker_pk_fe.iter() {
            packed = packed * Field::from(256u64) + Field::from(*byte);
        }
        packed
    }
}

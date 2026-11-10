use borsh::{BorshDeserialize, BorshSerialize};
use spr_program::{input, ClientInputs, UtxoOwner, UtxoState};

/// The escrow terms: the escrow UTXO's state. The SDK hashes them into its
/// data hash and encrypts them as its data.
#[input]
#[derive(Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, UtxoState, ClientInputs)]
pub struct EscrowTerms {
    /// The creator: owns the spent source and the change at escrow, signs the
    /// withdraw and receives the withdrawal.
    pub creator: UtxoOwner,
    /// Unix seconds; revealed at withdraw and checked against the Clock by
    /// the program.
    pub unlock: u64,
}

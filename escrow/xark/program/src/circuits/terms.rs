use spr_program::{input, poseidon, CircuitError, ClientInputs, Field};

/// The escrow terms the escrow UTXO commits as its data hash.
#[input]
#[derive(ClientInputs)]
pub struct EscrowTerms {
    /// The creator's shielded owner hash: receives the change at escrow and
    /// the withdrawal at withdraw.
    pub owner_hash: Field,
    /// Unix seconds; revealed at withdraw and checked against the Clock by
    /// the program.
    pub unlock: u64,
}

impl EscrowTerms {
    /// `Poseidon(owner_hash, unlock)`, the escrow UTXO's data hash.
    pub fn data_hash(&self) -> Result<Field, CircuitError> {
        poseidon(&[self.owner_hash, Field::from(self.unlock)])
    }
}

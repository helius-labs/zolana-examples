use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs, TokenUtxos, Utxo,
        UtxoTrait,
    },
    input, require, require_eq, CircuitError, ClientInputs, ShieldedAddress, TxContext, ZkProgram,
};

use super::EscrowTerms;

#[input]
#[derive(ZkProgram)]
pub struct Escrow {
    pub private: EscrowPrivateInputs,
    pub public: EscrowPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct EscrowPrivateInputs {
    pub tx_context: TxContext,
    pub terms: EscrowTerms,
    /// The amount locked in the escrow UTXO.
    pub amount: u64,
    /// The spent source UTXO. The circuit does not constrain its owner: SPP
    /// authorizes the spend.
    pub source: Utxo,
    /// The creator's shielded address, whose owner hash is
    /// `terms.owner_hash`; it receives the change.
    pub creator: ShieldedAddress,
    /// The escrow UTXO's owner. The circuit does not constrain it: SPP spends
    /// the escrow UTXO only with the escrow-authority PDA's signature.
    pub escrow_address: ShieldedAddress,
}

/// The escrow proof has no public field: its public hash commits only the
/// transaction (`private_tx_hash`).
#[input]
#[derive(PublicInputs)]
pub struct EscrowPublicInputs {}

impl Escrow {
    /// 1. Check the locked amount and the change owner.
    /// 2. Spend the source UTXO.
    /// 3. Create the change and the escrow UTXO, which commits the terms.
    /// 4. Lock `amount`; the rest is change.
    /// 5. Check the transaction: outputs `[change, escrow UTXO]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;

        // 1. Check the locked amount and the change owner.
        require(private.amount != 0);
        let creator = private.creator.owner();
        require_eq(creator.hash()?, private.terms.owner_hash);

        // 2. Spend the source UTXO.
        let mut source = TokenUtxos::new_mut(core::array::from_ref(&private.source))?.close();
        let asset = source.asset();

        // 3. Create the change and the escrow UTXO, which commits the terms.
        // A zero change is still a real UTXO, as the escrow output follows it.
        let mut change = TokenUtxos::new_init(creator, &asset).with_zero_output();
        let mut escrow = DataUtxo::new_init_with_hash(
            private.escrow_address.owner(),
            &private.terms.data_hash()?,
        )
        .with_asset(&asset)?;

        // 4. Lock `amount`; the rest is change.
        source.transfer(&mut escrow, &private.amount)?;
        source.transfer_all(&mut change)?;

        // 5. Check the transaction: outputs `[change, escrow UTXO]`.
        ConfidentialTransaction::new(&private.tx_context, &self.public)
            .with(source)
            .with(change)
            .with(escrow)
            .check()
    }
}

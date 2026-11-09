use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs, TokenUtxos, Utxo,
        UtxoTrait,
    },
    input, require, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext,
    ZkProgram,
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
    /// The spent source UTXO, owned by the creator.
    pub source: Utxo,
    /// The amount locked in the escrow UTXO; the rest of the source is change.
    pub amount: u64,
    /// Unix seconds; the escrow term `unlock`.
    pub unlock: u64,
    /// The escrow UTXO's owner. The circuit does not constrain it: SPP spends
    /// the escrow UTXO only with the escrow-authority PDA's signature.
    pub escrow_address: ShieldedAddress,
}

#[input]
#[derive(PublicInputs)]
pub struct EscrowPublicInputs {
    /// `hash_bytes` of the creator signer's tagged pubkey, which the program
    /// computes from the signer account.
    pub owner_pk_field: Field,
}

impl Escrow {
    /// 1. Spend the source and check the creator signs.
    /// 2. Lock `amount` in the escrow UTXO; the rest stays with the creator.
    /// 3. Set the escrow terms.
    /// 4. Check the transaction: outputs `[change, escrow UTXO]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;

        // 1. Spend the source and check the creator signs.
        require(private.amount != 0);
        let mut source = TokenUtxos::new_mut(core::array::from_ref(&private.source))?;
        require_eq(source.owner().key().identity()?, self.public.owner_pk_field);

        // 2. Lock `amount` in the escrow UTXO; the rest stays with the creator.
        let mut escrow = DataUtxo::<EscrowTerms>::new_init(&private.escrow_address)
            .with_asset(&source.asset())?;
        source.transfer(&mut escrow, &private.amount)?;

        // 3. Set the escrow terms.
        escrow.creator = source.owner();
        escrow.unlock = private.unlock;

        // 4. Check the transaction: outputs `[change, escrow UTXO]`.
        ConfidentialTransaction::new(&private.tx_context, &self.public)
            .with(source)
            .with(escrow)
            .check()
    }
}

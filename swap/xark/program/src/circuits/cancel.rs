use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, PublicInputs, TokenUtxos, Utxo, UtxoTrait,
    },
    input, require_eq, CircuitError, ClientInputs, Field, TxContext, ZkProgram,
};

use super::{shared::spend_order, OrderTerms};

#[input]
#[derive(ZkProgram)]
pub struct Cancel {
    pub private: CancelPrivateInputs,
    pub public: CancelPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct CancelPrivateInputs {
    pub tx_context: TxContext,
    pub terms: OrderTerms,
    /// The order UTXO, which commits `terms`.
    pub order_utxo: Utxo,
}

#[input]
#[derive(PublicInputs)]
pub struct CancelPublicInputs {
    /// The instruction's `order_expiry`; the program checks `now > expiry`.
    pub expiry: u64,
    /// `hash_bytes` of the maker signer's tagged pubkey, which the program
    /// computes from the signer account.
    pub maker_owner_pk_field: Field,
}

impl Cancel {
    /// 1. Check the public expiry.
    /// 2. Check the signer is the maker.
    /// 3. Spend the order UTXO, which commits the terms.
    /// 4. Refund the whole order to the maker.
    /// 5. Check the transaction: input `[order UTXO]`, output `[source]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let terms = &private.terms;

        // 1. Check the public expiry.
        require_eq(public.expiry, terms.expiry);

        // 2. The terms commit the maker's owner, whose signing key identity
        // the program computes from the signer.
        let maker = terms.maker.owner();
        require_eq(maker.key().identity()?, public.maker_owner_pk_field);

        // 3. Spend the order UTXO, which commits the terms.
        let mut order = spend_order(&private.order_utxo, terms)?;

        // 4. Refund the whole order to the maker.
        let mut source_output = TokenUtxos::new_init(maker, &order.asset());
        order.transfer_all(&mut source_output)?;

        // 5. Check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(order)
            .with(source_output)
            .check()
    }
}

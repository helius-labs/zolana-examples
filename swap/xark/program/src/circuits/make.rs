use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs, TokenUtxos, Utxo,
        UtxoTrait,
    },
    input, require, CircuitError, ClientInputs, ShieldedAddress, TxContext, ZkProgram,
};

use super::OrderTerms;

#[input]
#[derive(ZkProgram)]
pub struct Make {
    pub private: MakePrivateInputs,
    pub public: MakePublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct MakePrivateInputs {
    pub tx_context: TxContext,
    pub terms: OrderTerms,
    /// The amount locked in the order UTXO.
    pub source_amount: u64,
    /// The spent source UTXO. The circuit does not constrain its owner: SPP
    /// authorizes the spend.
    pub source: Utxo,
    /// The order UTXO's owner, the order-authority PDA with the taker's
    /// viewing key so the taker can decrypt the order. The circuit does not
    /// constrain it: SPP spends the order UTXO only with the PDA signature.
    pub order_address: ShieldedAddress,
}

/// The make proof has no public field: its public hash commits only the
/// transaction (`private_tx_hash`).
#[input]
#[derive(PublicInputs)]
pub struct MakePublicInputs {}

impl Make {
    /// 1. Check the order terms and the locked amount.
    /// 2. Spend the source UTXO.
    /// 3. Create the change to the maker and the order UTXO, which commits the
    ///    terms.
    /// 4. Lock `source_amount`; the rest is change.
    /// 5. Check the transaction: outputs `[change, order UTXO]`.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;

        // 1. Check the order terms and the locked amount.
        private.terms.assert_valid();
        require(private.source_amount != 0);

        // 2. Spend the source UTXO.
        let mut source = TokenUtxos::new_mut(core::array::from_ref(&private.source))?.close();
        let asset = source.asset();

        // 3. Create the change and the order UTXO. A zero change is still a
        // real UTXO, as the order UTXO follows it.
        let mut change =
            TokenUtxos::new_init(private.terms.maker.owner(), &asset).with_zero_output();
        let mut order =
            DataUtxo::<OrderTerms>::new_init(private.order_address.owner()).with_asset(&asset)?;
        *order = private.terms;

        // 4. Lock `source_amount`; the rest is change.
        source.transfer(&mut order, &private.source_amount)?;
        source.transfer_all(&mut change)?;

        // 5. Check the transaction: outputs `[change, order UTXO]`.
        ConfidentialTransaction::new(&private.tx_context, &self.public)
            .with(source)
            .with(change)
            .with(order)
            .check()
    }
}

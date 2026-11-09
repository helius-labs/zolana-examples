use spr_program::{
    circuit::{
        CheckedTransaction, ConfidentialTransaction, PublicInputs, TokenUtxos, Utxo, UtxoTrait,
    },
    input, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext, ZkProgram,
};

use super::{
    shared::{assert_blinding_seed, spend_order},
    OrderTerms,
};

#[input]
#[derive(ZkProgram)]
pub struct Take {
    pub private: TakePrivateInputs,
    pub public: TakePublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct TakePrivateInputs {
    pub tx_context: TxContext,
    pub terms: OrderTerms,
    /// The order UTXO, which commits `terms`.
    pub order_utxo: Utxo,
    /// The taker's destination-side input. Its owner receives the source.
    pub taker_in: Utxo,
    /// The maker's shielded address, whose viewing key the maker payout is
    /// encrypted to. The circuit does not read it: the payout owner is the
    /// committed `terms.maker`.
    pub maker_address: ShieldedAddress,
}

#[input]
#[derive(PublicInputs)]
pub struct TakePublicInputs {
    /// The transact's `expiry_unix_ts`; the program checks `now <= expiry`.
    pub expiry: u64,
    /// The first SPP input's nullifier, which the program reads from the
    /// transact.
    pub first_nullifier: Field,
}

impl Take {
    /// 1. Check the blinding seed derives from the order opening.
    /// 2. Spend the order UTXO, which commits the terms.
    /// 3. Check the order terms and the public expiry.
    /// 4. Spend the taker input of the destination asset and check the
    ///    designated taker.
    /// 5. Create the source output to the taker and the destination output to
    ///    the maker.
    /// 6. Pay the whole order to the taker and exactly `destination_amount` to
    ///    the maker.
    /// 7. Check the transaction: inputs `[order UTXO, taker input]`, outputs
    ///    `[source, destination]`.
    /// 8. Check the first nullifier the output blindings derive from is the
    ///    published one.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;

        // 1. Both parties hold the order opening. With the published first
        // nullifier, the seed makes every payout recoverable without the
        // taker's ciphertext.
        assert_blinding_seed(
            &private.tx_context,
            crate::TAKE_BLINDING_DOMAIN,
            &private.order_utxo.blinding,
        )?;

        // 2. Spend the order UTXO, which commits the terms.
        let mut order = spend_order(&private.order_utxo, &private.terms)?;

        // 3. Check the order terms and the public expiry.
        order.assert_valid();
        require_eq(public.expiry, order.expiry);

        // 4. Spend the taker input and check the designated taker.
        let mut taker_in = TokenUtxos::new_mut(core::array::from_ref(&private.taker_in))?.close();
        require_eq(taker_in.asset(), order.destination_mint);
        let taker = taker_in.owner();
        order.assert_taker(&taker)?;

        // 5. Create the source and destination outputs.
        let mut source_output = TokenUtxos::new_init(&taker, &order.asset());
        let mut destination_output = TokenUtxos::new_init(&order.maker, &order.destination_mint);

        // 6. The closed taker input must end at zero, so it holds exactly the
        // destination amount.
        let destination_amount = order.destination_amount;
        order.transfer_all(&mut source_output)?;
        taker_in.transfer(&mut destination_output, &destination_amount)?;

        // 7. Check the transaction.
        let checked = ConfidentialTransaction::new(&private.tx_context, public)
            .with(order)
            .with(taker_in)
            .with(source_output)
            .with(destination_output)
            .check()?;

        // 8. Check the published first nullifier.
        require_eq(checked.first_nullifier, public.first_nullifier);
        Ok(checked)
    }
}

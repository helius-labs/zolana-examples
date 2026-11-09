use spr_program::{
    circuit::PublicInputs, input, poseidon, require_eq, CircuitError, Field, ZkProgram,
};

use super::{
    shared::{check_private_tx_hash, check_transaction_blindings, Commitment},
    OrderTerms, ProofInputUtxo,
};

#[input]
#[derive(ZkProgram)]
#[zk_program(raw_client)]
pub struct Take {
    pub private: TakePrivateInputs,
    pub public: TakePublicInputs,
}

#[input]
pub struct TakePrivateInputs {
    pub private_tx_hash: Field,
    pub order: OrderTerms,
    pub order_utxo: ProofInputUtxo,
    /// The taker's destination-side input.
    pub taker_in: ProofInputUtxo,
    /// Output slot 0: the source funds to the taker.
    pub source_output: ProofInputUtxo,
    /// Output slot 1: the destination funds to the maker.
    pub destination_output: ProofInputUtxo,
    pub private_tx_blinding: Field,
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
    /// 1. Check the blinding seed and the transaction blindings.
    /// 2. Check the order terms and the public expiry.
    /// 3. Check the order UTXO input commits the terms.
    /// 4. Check the taker input pays the destination amount.
    /// 5. Check the source output pays the order to the taker.
    /// 6. Check the destination output pays the maker.
    /// 7. Check the transaction: inputs `[order UTXO, taker input]`, outputs
    ///    `[source, destination]`.
    pub fn execute(&self) -> Result<Commitment, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let order = &private.order;
        let order_utxo = &private.order_utxo;
        let taker_in = &private.taker_in;
        let source = &private.source_output;
        let destination = &private.destination_output;

        // 1. Both parties hold the order opening. Binding the seed and the
        // published first nullifier makes every payout recoverable without
        // the taker's ciphertext.
        let blinding_seed = poseidon(&[
            Field::from(crate::TAKE_BLINDING_DOMAIN),
            order_utxo.blinding,
        ])?;
        check_transaction_blindings(
            public.first_nullifier,
            blinding_seed,
            private.private_tx_blinding,
            &[source.blinding, destination.blinding],
        )?;

        // 2. Check the order terms and the public expiry.
        order.check();
        require_eq(public.expiry, order.expiry);

        // 3. Check the order UTXO input commits the terms.
        let order_utxo_hash = order.check_order_utxo(order_utxo)?;

        // 4. Check the taker input pays the destination amount.
        let destination_amount = Field::from(order.destination_amount);
        taker_in.assert_plain();
        require_eq(taker_in.asset, order.destination_asset);
        require_eq(taker_in.amount, destination_amount);

        // 5. Check the source output pays the order to the taker.
        source.assert_plain();
        require_eq(source.asset, order_utxo.asset);
        require_eq(source.amount, order_utxo.amount);
        require_eq(source.owner_hash, taker_in.owner_hash);

        // 6. Check the destination output pays the maker.
        destination.assert_plain();
        require_eq(destination.asset, order.destination_asset);
        require_eq(destination.amount, destination_amount);
        require_eq(destination.owner_hash, order.maker_owner_hash);

        // 7. Check the transaction.
        check_private_tx_hash(
            &[order_utxo_hash, taker_in.hash()?],
            &[source.hash()?, destination.hash()?],
            private.private_tx_blinding,
            private.private_tx_hash,
        )?;
        Ok(Commitment(public.hash(&private.private_tx_hash)?))
    }
}

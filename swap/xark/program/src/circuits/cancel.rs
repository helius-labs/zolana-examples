use spr_program::{
    circuit::PublicInputs, input, poseidon, require_eq, CircuitError, Field, ZkProgram,
};

use super::{
    shared::{check_private_tx_hash, Commitment},
    OrderTerms, ProofInputUtxo,
};

#[input]
#[derive(ZkProgram)]
#[zk_program(raw_client)]
pub struct Cancel {
    pub private: CancelPrivateInputs,
    pub public: CancelPublicInputs,
}

#[input]
pub struct CancelPrivateInputs {
    pub private_tx_hash: Field,
    pub order: OrderTerms,
    pub order_utxo: ProofInputUtxo,
    /// The refund of the order to the maker.
    pub source_output: ProofInputUtxo,
    /// With the public `maker_owner_pk_field`, the preimage of the committed
    /// `maker_owner_hash`.
    pub maker_nullifier_pk: Field,
    pub private_tx_blinding: Field,
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
    /// 2. Check the order UTXO input commits the terms.
    /// 3. Check the source output refunds the order to the maker.
    /// 4. Check the signer is the maker.
    /// 5. Check the transaction: input `[order UTXO]`, output `[source]`; the
    ///    compact padding output does not enter the private transaction hash.
    pub fn execute(&self) -> Result<Commitment, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let order = &private.order;
        let order_utxo = &private.order_utxo;
        let source = &private.source_output;

        // 1. Check the public expiry.
        require_eq(public.expiry, order.expiry);

        // 2. Check the order UTXO input commits the terms.
        let order_utxo_hash = order.check_order_utxo(order_utxo)?;

        // 3. Check the source output refunds the order to the maker.
        source.assert_plain();
        require_eq(source.asset, order_utxo.asset);
        require_eq(source.amount, order_utxo.amount);
        require_eq(source.owner_hash, order.maker_owner_hash);

        // 4. Check the signer is the maker.
        require_eq(
            poseidon(&[public.maker_owner_pk_field, private.maker_nullifier_pk])?,
            order.maker_owner_hash,
        );

        // 5. Check the transaction.
        check_private_tx_hash(
            &[order_utxo_hash],
            &[source.hash()?],
            private.private_tx_blinding,
            private.private_tx_hash,
        )?;
        Ok(Commitment(public.hash(&private.private_tx_hash)?))
    }
}

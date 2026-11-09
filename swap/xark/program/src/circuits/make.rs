use spr_program::{circuit::PublicInputs, input, require_eq, CircuitError, Field, ZkProgram};

use super::{
    shared::{check_private_tx_hash, Commitment},
    OrderTerms, ProofInputUtxo,
};

#[input]
#[derive(ZkProgram)]
#[zk_program(raw_client)]
pub struct Make {
    pub private: MakePrivateInputs,
    pub public: MakePublicInputs,
}

#[input]
pub struct MakePrivateInputs {
    pub private_tx_hash: Field,
    pub order: OrderTerms,
    pub order_utxo: ProofInputUtxo,
    pub change: ProofInputUtxo,
    /// The maker's source input. A free witness: SPP proves its ownership
    /// and inclusion.
    pub source_input_hash: Field,
    pub private_tx_blinding: Field,
}

/// The make proof has no public field: its public hash commits only the
/// transaction (`private_tx_hash`).
#[input]
#[derive(PublicInputs)]
pub struct MakePublicInputs {}

impl Make {
    /// 1. Check the order terms.
    /// 2. Check the order UTXO output commits the terms.
    /// 3. Check the change output returns the order asset to the maker.
    /// 4. Check the transaction: inputs `[source, padding]`, outputs
    ///    `[change, order UTXO]`.
    pub fn execute(&self) -> Result<Commitment, CircuitError> {
        let private = &self.private;
        let order = &private.order;

        // 1. Check the order terms.
        order.check();

        // 2. Check the order UTXO output commits the terms.
        let order_utxo_hash = order.check_order_utxo(&private.order_utxo)?;

        // 3. Check the change output returns the order asset to the maker.
        let change = &private.change;
        change.assert_plain();
        require_eq(change.asset, private.order_utxo.asset);
        require_eq(change.owner_hash, order.maker_owner_hash);

        // 4. Check the transaction. A zero change is still a real output.
        check_private_tx_hash(
            &[private.source_input_hash, Field::zero()],
            &[change.hash()?, order_utxo_hash],
            private.private_tx_blinding,
            private.private_tx_hash,
        )?;
        Ok(Commitment(self.public.hash(&private.private_tx_hash)?))
    }
}

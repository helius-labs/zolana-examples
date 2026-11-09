use spr_program::{
    circuit::{DataUtxo, Utxo, UtxoTrait},
    poseidon, require, require_eq, CircuitError, Field, TxContext,
};

use super::OrderTerms;

/// The transaction's blinding seed is `Poseidon(tag, order_blinding)`, so
/// anyone holding the order opening derives every output blinding from the
/// seed and the first nullifier.
pub(super) fn assert_blinding_seed(
    context: &TxContext,
    tag: u32,
    order_blinding: &Field,
) -> Result<(), CircuitError> {
    require_eq(
        context.blinding_seed,
        poseidon(&[Field::from(tag), *order_blinding])?,
    );
    Ok(())
}

/// Spends the order UTXO, which commits `terms` and holds a nonzero amount,
/// and leaves no output. The owner stays a free witness: SPP spends the order
/// UTXO only with the order-authority PDA signature.
pub(super) fn spend_order(
    input: &Utxo,
    terms: &OrderTerms,
) -> Result<DataUtxo<OrderTerms>, CircuitError> {
    let order = DataUtxo::new_mut(input, terms)?.close();
    require(order.amount()? != 0);
    Ok(order)
}

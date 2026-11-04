use zolana_program::{
    circuit::{DataUtxo, Utxo, UtxoTrait},
    poseidon, require_eq, CircuitError, Field, ShieldedAddress, TxContext, UtxoOwner,
};

/// Binds the private DvP address to the public PDA owner hash the program
/// derives, and returns the owner the circuit checks UTXOs against.
pub(super) fn dvp_owner<'a>(
    address: &'a ShieldedAddress,
    owner_hash: &Field,
) -> Result<&'a UtxoOwner, CircuitError> {
    let owner = address.owner();
    require_eq(owner.hash()?, *owner_hash);
    Ok(owner)
}

pub(super) fn assert_blinding_seed(
    context: &TxContext,
    tag: u32,
    terms_blinding: &Field,
) -> Result<(), CircuitError> {
    require_eq(
        context.blinding_seed,
        poseidon(&[Field::from(tag), *terms_blinding])?,
    );
    Ok(())
}

pub(super) fn close_dvp_state(
    input: &Utxo,
    owner: &UtxoOwner,
    dvp_id: &Field,
) -> Result<DataUtxo<Field>, CircuitError> {
    let state = DataUtxo::new_mut_with_hash(input, dvp_id)?.close();
    require_eq(state.owner(), *owner);
    require_eq(state.asset(), [0u8; 32]);
    require_eq(state.amount()?, 0u64);
    Ok(state)
}

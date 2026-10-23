use zolana_program::{
    circuit::{
        constant, poseidon, zero, Assert, CircuitVar, DataUtxo, SolanaAddress, TxContext, Utxo,
        UtxoOwner, UtxoTrait,
    },
    CircuitError,
};

pub(super) fn assert_blinding_seed(
    context: &TxContext,
    tag: u32,
    terms_blinding: &CircuitVar,
) -> Result<(), CircuitError> {
    context.blinding_seed.assert_equal(
        &poseidon(&[constant(u64::from(tag)), terms_blinding.clone()])?,
        "the transaction seed derives from the DvP terms and instruction",
    )
}
pub(super) fn close_dvp_state(
    input: &Utxo,
    owner: &UtxoOwner,
    dvp_id: &CircuitVar,
) -> Result<DataUtxo<CircuitVar>, CircuitError> {
    let state = DataUtxo::new_mut_with_hash(input, dvp_id)?.close();
    state
        .owner()
        .assert_equal(owner, "the DvP PDA owns its state")?;
    state.asset().assert_equal(
        &SolanaAddress::constant(&zolana_transaction::SOL_MINT),
        "the DvP state uses SOL",
    )?;
    state
        .amount()?
        .var()
        .assert_equal(&zero(), "the DvP state holds no funds")?;
    Ok(state)
}

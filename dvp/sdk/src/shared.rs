use anyhow::Result;
use zolana_transaction::{
    utxo::{derive_output_blinding_seed, derive_transact_output_blinding},
    SppProofOutputUtxo,
};

use crate::err;

/// Gives every output, compact padding included, the blinding SPP derives
/// for its slot from `blinding_seed` and the transaction's first nullifier:
/// SPP checks each output blinding against that derivation.
pub(crate) fn assign_output_blindings(
    outputs: &mut [SppProofOutputUtxo],
    first_nullifier: &[u8; 32],
    blinding_seed: &[u8; 32],
) -> Result<()> {
    let seed = derive_output_blinding_seed(first_nullifier, blinding_seed).map_err(err)?;
    for (index, output) in (0u32..).zip(outputs) {
        output.blinding =
            derive_transact_output_blinding(first_nullifier, &seed, index).map_err(err)?;
    }
    Ok(())
}

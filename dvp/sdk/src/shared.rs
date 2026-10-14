use anyhow::{ensure, Result};
use zolana_client::ZolanaIndexer;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_keypair::NullifierKey;
use zolana_transaction::{
    instructions::transact::SppProofInputs,
    utxo::{derive_output_blinding_seed, derive_transact_output_blinding},
    SppProofOutputUtxo,
};

use crate::err;

/// The public zero-secret nullifier key of every PDA-owned DvP UTXO.
pub(crate) fn zero_nullifier_key() -> NullifierKey {
    NullifierKey::from_secret([0u8; 31])
}

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

/// Proves a transaction whose real inputs are all owned by the DvP PDA under
/// the zero nullifier secret (padding inputs carry their own) through
/// `indexer`, which serves their Merkle proofs, and checks that the SPP
/// proof covers `private_tx_hash`, the transaction the DvP proof covers.
pub(crate) fn prove_pda_transact(
    indexer: &ZolanaIndexer,
    spp_proof_inputs: SppProofInputs,
    private_tx_hash: &[u8; 32],
) -> Result<TransactIxData> {
    let spp = indexer
        .prove_transact(spp_proof_inputs, &zero_nullifier_key())
        .map_err(err)?;
    ensure!(
        &spp.private_tx_hash == private_tx_hash,
        "the SPP proof and the DvP proof cover different transactions"
    );
    Ok(spp)
}

/// The instruction builders read every input from the output tree, so an
/// input `what` must sit in the tree with id `tree_id`.
pub(crate) fn ensure_output_tree(what: &str, input_tree_id: u16, tree_id: u16) -> Result<()> {
    ensure!(
        input_tree_id == tree_id,
        "{what} is in tree {input_tree_id}, not in the output tree {tree_id}"
    );
    Ok(())
}

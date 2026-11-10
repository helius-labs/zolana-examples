use anyhow::{ensure, Result};
use zolana_client::ZolanaIndexer;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_program::ProgramOwner;
use zolana_transaction::instructions::transact::SppProofInputs;

use crate::err;

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
        .prove_transact(spp_proof_inputs, &ProgramOwner::nullifier_key())
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

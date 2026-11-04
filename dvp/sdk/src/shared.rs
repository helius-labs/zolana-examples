use anyhow::{ensure, Result};
use zolana_client::ZolanaIndexer;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_program::ProgramOwner;
use zolana_transaction::instructions::transact::SppProofInputs;

use zolana_hasher::primitives::solana_owner_identity;

use crate::err;

pub(crate) fn field_decimal(bytes: [u8; 32]) -> Result<String> {
    Ok(zolana_program::from_bytes(&bytes)
        .map_err(err)?
        .to_decimal())
}

/// A Solana address's owner identity, as a decimal proof input.
pub(crate) fn identity_decimal(address: &solana_address::Address) -> Result<String> {
    field_decimal(solana_owner_identity(address.as_array()).map_err(err)?)
}

/// Runs a program transaction's `prepare`, whose native circuit `execute`
/// panics when an Xark `require` fails, and returns that failure as an error.
pub(crate) fn catch_requirement<T>(prepare: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(prepare)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("the circuit rejects these values");
        Err(anyhow::anyhow!("{message}"))
    })
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

#[cfg(test)]
mod tests {
    use super::catch_requirement;

    #[test]
    fn catch_requirement_returns_a_failed_requirement_as_an_error() {
        // 1. A native Xark requirement failure panics inside prepare.
        let error = catch_requirement(|| -> anyhow::Result<()> {
            zolana_program::require(false);
            Ok(())
        })
        .expect_err("a failed requirement is an error");
        assert!(error.to_string().contains("requirement failed"), "{error}");
        // 2. Successful and fallible results pass through unchanged.
        assert_eq!(catch_requirement(|| Ok(7)).unwrap(), 7);
        let failed = catch_requirement(|| -> anyhow::Result<()> { anyhow::bail!("rejected") });
        assert_eq!(failed.unwrap_err().to_string(), "rejected");
    }
}

use anyhow::{ensure, Result};
use solana_address::Address;
use spr_client::{ProofAuthority, SprIndexer, TransferInput};
use spr_hasher::primitives::solana_owner_identity;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use spr_keypair::ShieldedKeypair;
use spr_program::{Field, ProgramOwner};
use spr_transaction::instructions::transact::SppProofInputs;

use crate::err;

/// Raw id of the tree a rediscovered leaf was appended to. The tree id is the
/// second element of a UTXO commitment, so the indexer must hash a candidate
/// opening under the same id as the leaf it compares against. The examples
/// use the localnet's single default tree, whose raw id is 0.
pub const INDEXED_TREE_ID: u16 = 0;

pub(crate) fn field(bytes: [u8; 32]) -> Result<Field> {
    spr_program::from_bytes(&bytes).map_err(err)
}

/// A Solana address's owner identity, `hash_bytes` of its tagged pubkey: the
/// value the program computes from a signer and an order's `taker_pk_fe`.
pub fn identity(address: &Address) -> Result<[u8; 32]> {
    solana_owner_identity(address.as_array()).map_err(err)
}

/// Proves an SPP transaction through `indexer`, which serves the Merkle
/// proofs, completing the inputs with `authority`, and checks that the SPP
/// proof covers the transaction the swap proof covers.
pub(crate) fn prove_transact(
    indexer: &SprIndexer,
    spp_proof_inputs: SppProofInputs,
    authority: &dyn ProofAuthority,
) -> Result<TransactIxData> {
    let private_tx_hash = spp_proof_inputs
        .padding_independent_private_tx_hash()
        .map_err(err)?;
    let spp = indexer
        .prove_transact(spp_proof_inputs, authority)
        .map_err(err)?;
    ensure!(
        spp.private_tx_hash == private_tx_hash,
        "the SPP proof and the swap proof cover different transactions"
    );
    Ok(spp)
}

/// The take's proof authority: the order UTXO is owned by the order-authority
/// PDA under the zero nullifier key, the taker input by the taker.
pub struct TakeAuthority<'a>(pub &'a ShieldedKeypair);

impl ProofAuthority for TakeAuthority<'_> {
    fn complete_inputs(&self, inputs: &mut [TransferInput]) -> Result<(), spr_client::ClientError> {
        for input in inputs.iter_mut() {
            if input.nullifier_secret.is_some() {
                continue;
            }
            let input = core::slice::from_mut(input);
            // The zero key refuses an input whose nullifier it does not derive.
            if ProgramOwner::nullifier_key()
                .complete_inputs(input)
                .is_err()
            {
                self.0.complete_inputs(input)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_blinding(byte: u8) -> [u8; 32] {
    let mut blinding = [byte; 32];
    blinding[0] = 0;
    blinding
}

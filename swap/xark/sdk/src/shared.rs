use anyhow::Result;
use solana_address::Address;
use spr_keypair::ShieldedAddress;
use spr_program::{from_bytes, Field};
use spr_transaction::{
    instructions::transact::SppProofOutputUtxo,
    utxo::{ProofInputUtxo, SppProofInputUtxo},
};
use swap_program::circuits;

use crate::err;

/// Raw id of the tree a rediscovered leaf was appended to. The tree id is the
/// second element of a UTXO commitment, so the indexer must hash a candidate
/// opening under the same id as the leaf it compares against. The examples
/// use the localnet's single default tree, whose raw id is 0.
pub const INDEXED_TREE_ID: u16 = 0;

/// A 32-byte hash, identity or blinding as a proof input.
pub(crate) fn field(bytes: &[u8; 32]) -> Result<Field> {
    from_bytes(bytes).map_err(err)
}

/// A UTXO hash preimage as the swap circuits take it.
pub(crate) fn circuit_utxo(utxo: &ProofInputUtxo) -> Result<circuits::ProofInputUtxo> {
    Ok(circuits::ProofInputUtxo {
        domain: field(&utxo.domain)?,
        tree_id: field(&utxo.tree_id)?,
        owner_hash: field(&utxo.owner_hash)?,
        asset: field(&utxo.asset)?,
        amount: field(&utxo.amount)?,
        blinding: field(&utxo.blinding)?,
        data_hash: field(&utxo.data_hash)?,
        ring_data_hash: field(&utxo.ring_data_hash)?,
        ring_program_id: field(&utxo.ring_program_id)?,
    })
}

pub fn input_sum(inputs: &[SppProofInputUtxo], asset: &Address) -> i128 {
    inputs
        .iter()
        .filter(|input_utxo| &input_utxo.utxo.asset.asset == asset)
        .map(|input_utxo| i128::from(input_utxo.utxo.amount))
        .sum()
}

#[cfg(test)]
pub(crate) fn test_blinding(byte: u8) -> spr_transaction::utxo::Blinding {
    let mut blinding = [byte; 32];
    blinding[0] = 0;
    blinding
}

pub(crate) fn check_output_utxo(
    label: &str,
    output: &SppProofOutputUtxo,
    mint: &Address,
    amount: u64,
) -> Result<ShieldedAddress> {
    let owner = output
        .owner_address
        .ok_or_else(|| err(format!("{label} owner address missing")))?;
    if &output.asset.asset != mint {
        return Err(err(format!("{label} asset mismatch")));
    }
    if output.amount != amount {
        return Err(err(format!("{label} amount mismatch")));
    }
    if output.data_hash.is_some()
        || output.ring_data_hash.is_some()
        || output.ring_program_id.is_some()
    {
        return Err(err(format!(
            "{label} must not carry data or ring commitments"
        )));
    }
    Ok(owner)
}

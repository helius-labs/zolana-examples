use anyhow::Result;
use solana_address::Address;
use spr_keypair::ShieldedAddress;
use spr_transaction::{instructions::transact::SppProofOutputUtxo, utxo::SppProofInputUtxo};

use crate::err;

/// Raw id of the tree a rediscovered leaf was appended to. The tree id is the
/// second element of a UTXO commitment, so the indexer must hash a candidate
/// opening under the same id as the leaf it compares against.
// TODO(tree-id): resolve the tree id from the tree account holding the leaf.
pub const INDEXED_TREE_ID: u16 = 0;

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

/// `utxo` as an input owned by `key`, committed under `tree_id` at leaf 0 with
/// no data or ring data hash.
#[cfg(test)]
pub(crate) fn test_input_utxo(
    utxo: spr_transaction::utxo::Utxo,
    key: &spr_keypair::NullifierKey,
    tree_id: u16,
) -> SppProofInputUtxo {
    let nullifier_pubkey = key.pubkey().expect("nullifier pubkey");
    let utxo_hash = utxo
        .hash(&nullifier_pubkey, &[0; 32], &[0; 32], tree_id)
        .expect("input hash");
    let nullifier = key
        .nullifier(&utxo_hash, &utxo.blinding)
        .expect("input nullifier");
    spr_transaction::WalletUtxo {
        utxo,
        nullifier_pubkey,
        utxo_hash,
        nullifier,
        tx_viewing_key: None,
        data_hash: None,
        ring_data_hash: None,
        tree_id,
        leaf_index: 0,
        latest_tree_id: None,
        slot: 0,
        tx_signature: Default::default(),
        slot_index: 0,
    }
    .into()
}

/// The proof inputs `sender` posts spending `input_utxos` into `output_utxos`
/// under a random blinding seed and salt: each output blinding derived from
/// the first input's nullifier, each output encrypted to its owner under the
/// transaction viewing key and published under `output_tree_id`.
#[cfg(test)]
pub(crate) fn test_transaction(
    input_utxos: Vec<SppProofInputUtxo>,
    mut output_utxos: Vec<SppProofOutputUtxo>,
    sender: &spr_keypair::ShieldedKeypair,
    output_tree_id: u16,
) -> spr_transaction::instructions::transact::SppProofInputs {
    use spr_interface::instruction::{OwnerTag, TransactOutput};
    use spr_transaction::{
        serialization::{
            confidential::{Confidential, ConfidentialEncode, ConfidentialOutputPlaintext},
            UtxoSerialization,
        },
        utxo::{derive_output_blinding_seed, derive_transact_output_blinding},
    };

    let blinding_seed = spr_keypair::random_blinding();
    let salt = spr_keypair::random_salt();
    let first_nullifier = input_utxos.first().expect("first input").nullifier;
    let seed = derive_output_blinding_seed(&first_nullifier, &blinding_seed)
        .expect("output blinding seed");
    for (index, output) in (0u32..).zip(output_utxos.iter_mut()) {
        output.blinding = derive_transact_output_blinding(&first_nullifier, &seed, index)
            .expect("output blinding");
    }
    let tx = sender
        .get_transaction_viewing_key(&first_nullifier)
        .expect("transaction viewing key");
    let mut outputs = Vec::with_capacity(output_utxos.len());
    let mut tags = Vec::with_capacity(output_utxos.len());
    for (slot_index, output) in (0u32..).zip(output_utxos.iter()) {
        let address = output.owner_address.expect("output owner");
        let tag = address
            .signing_pubkey
            .confidential_view_tag()
            .expect("view tag");
        let message = Confidential::encode_plaintext(
            &ConfidentialOutputPlaintext {
                asset_id: output.asset.asset_id,
                amount: output.amount,
                blinding: output.blinding,
                ring_program_id: output.ring_program_id,
                data: output.data.clone(),
            },
            tag,
            &ConfidentialEncode {
                tx: tx.clone(),
                recipient_pubkey: address.viewing_pubkey,
                salt,
                slot_index,
            },
        )
        .expect("encrypted output");
        outputs.push(TransactOutput {
            utxo_hash: output.hash(output_tree_id).expect("output hash"),
            owner_tag: OwnerTag::Inline(tag),
            data: Some(message.data),
        });
        tags.push(tag);
    }
    spr_transaction::instructions::transact::SppProofInputs {
        input_utxos,
        output_utxos,
        blinding_seed,
        output_tree_id,
        external_data: spr_transaction::ExternalData::new(
            *tx.pubkey().as_bytes(),
            salt,
            outputs,
            tags,
            vec![],
        ),
        payer: Address::default(),
        cache_accounts: Default::default(),
    }
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

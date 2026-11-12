//! Transaction-building helpers shared by the swap, cancel and benchmark tests:
//! output blindings, the transaction viewing key, output encryption and the
//! nullifier keys a proof completes its inputs with.

use anyhow::{anyhow, Result};
use num_bigint::BigUint;
use spr_client::{ClientError, ProofAuthority, TransferInput};
use spr_interface::instruction::{OwnerTag, TransactOutput};
use spr_keypair::{random_blinding, random_salt, NullifierKey, ShieldedKeypair, ViewingKey};
use spr_transaction::{
    instructions::transact::SppProofOutputUtxo,
    serialization::{
        confidential::{Confidential, ConfidentialEncode, ConfidentialOutputPlaintext},
        UtxoSerialization,
    },
    utxo::{derive_output_blinding_seed, derive_transact_output_blinding, SppProofInputUtxo},
};

/// The outputs of a transaction encrypted to their owners, with what
/// [`ExternalData::new`](spr_transaction::instructions::transact::ExternalData::new)
/// takes besides the transaction viewing key.
pub struct EncryptedTransactionData {
    pub salt: [u8; 16],
    pub output_utxos: Vec<SppProofOutputUtxo>,
    pub outputs: Vec<TransactOutput>,
    pub resolved_owner_tags: Vec<[u8; 32]>,
}

/// Set each output's blinding from the first nullifier and the output
/// blinding seed, by slot index.
pub fn assign_output_blindings(
    outputs: &mut [SppProofOutputUtxo],
    first_nullifier: &[u8; 32],
    seed: &[u8; 32],
) -> Result<()> {
    for (index, output) in (0u32..).zip(outputs.iter_mut()) {
        output.blinding = derive_transact_output_blinding(first_nullifier, seed, index)?;
    }
    Ok(())
}

/// Draw a random blinding seed and assign every output blinding from it.
/// Returns the seed, which goes into `SppProofInputs::blinding_seed`.
pub fn prepare_output_blindings(
    input_utxos: &[SppProofInputUtxo],
    outputs: &mut [SppProofOutputUtxo],
) -> Result<[u8; 32]> {
    let blinding_seed = random_blinding();
    let first_nullifier = first_nullifier(input_utxos)?;
    let seed = derive_output_blinding_seed(&first_nullifier, &blinding_seed)?;
    assign_output_blindings(outputs, &first_nullifier, &seed)?;
    Ok(blinding_seed)
}

/// The viewing key of the transaction spending `input_utxos`, derived from the
/// first input's nullifier.
pub fn get_transaction_viewing_key(
    keypair: &ShieldedKeypair,
    input_utxos: &[SppProofInputUtxo],
) -> Result<ViewingKey> {
    Ok(keypair.get_transaction_viewing_key(&first_nullifier(input_utxos)?)?)
}

fn first_nullifier(input_utxos: &[SppProofInputUtxo]) -> Result<[u8; 32]> {
    Ok(input_utxos
        .first()
        .ok_or_else(|| anyhow!("the transaction has no inputs"))?
        .nullifier)
}

/// Encrypt every output to its own owner under a random salt, and publish its
/// commitment under `output_tree_id`.
pub fn encrypt_transaction_data(
    outputs: &[SppProofOutputUtxo],
    transaction_viewing_key: &ViewingKey,
    output_tree_id: u16,
) -> Result<EncryptedTransactionData> {
    let salt = random_salt();
    let mut transact_outputs = Vec::with_capacity(outputs.len());
    let mut resolved_owner_tags = Vec::with_capacity(outputs.len());
    for (slot_index, output) in (0u32..).zip(outputs) {
        let address = output
            .owner_address
            .ok_or_else(|| anyhow!("output {slot_index} has no owner"))?;
        let message = Confidential::encode_plaintext(
            &ConfidentialOutputPlaintext {
                asset_id: output.asset.asset_id,
                amount: output.amount,
                blinding: output.blinding,
                ring_program_id: output.ring_program_id,
                data: output.data.clone(),
            },
            address.signing_pubkey.confidential_view_tag()?,
            &ConfidentialEncode {
                tx: transaction_viewing_key.clone(),
                recipient_pubkey: address.viewing_pubkey,
                salt,
                slot_index,
            },
        )?;
        transact_outputs.push(TransactOutput {
            utxo_hash: output.hash(output_tree_id)?,
            owner_tag: OwnerTag::Inline(message.view_tag),
            data: Some(message.data),
        });
        resolved_owner_tags.push(message.view_tag);
    }
    Ok(EncryptedTransactionData {
        salt,
        output_utxos: outputs.to_vec(),
        outputs: transact_outputs,
        resolved_owner_tags,
    })
}

/// Complete each input with the key whose nullifier it carries.
// The cancel test spends only the order note, with the program's own key.
#[allow(dead_code)]
pub struct ProofKeys<'a>(pub &'a [&'a NullifierKey]);

impl ProofAuthority for ProofKeys<'_> {
    fn complete_inputs(&self, inputs: &mut [TransferInput]) -> Result<(), ClientError> {
        for (index, input) in inputs.iter_mut().enumerate() {
            if input.nullifier_secret.is_some() {
                continue;
            }
            let hash = input.utxo.hash()?;
            let key = self
                .0
                .iter()
                .find(|key| {
                    key.nullifier(&hash, &input.utxo.blinding)
                        .is_ok_and(|nullifier| {
                            BigUint::from_bytes_be(&nullifier) == input.nullifier
                        })
                })
                .ok_or(ClientError::InputNullifierMismatch { index })?;
            key.complete_inputs(std::slice::from_mut(input))?;
        }
        Ok(())
    }
}

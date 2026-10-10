//! A confidential zolana transfer as either party builds it for a swap, and
//! the receiving side: decrypting the outputs addressed to a key and
//! checking each against its on-chain commitment before it is trusted.

use anyhow::{anyhow, Result};
use borsh::BorshDeserialize;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_client::{ProofAuthority, Rpc, ZolanaClient};
use zolana_event::OutputDataEncoding;
use zolana_interface::instruction::TransactIxData;
use zolana_keypair::{constants::P256_PUBKEY_LEN, P256Pubkey, ShieldedAddress};
use zolana_program::instruction::Transact;
use zolana_transaction::{
    instructions::transact::ConfidentialTransaction,
    serialization::confidential::{Confidential, ConfidentialOutputPlaintext},
    AssetRegistry, DecryptLabel, DecryptRequest, EncryptedScheme, ShieldedKeys, Utxo, WalletUtxo,
};

use zolana_client::{Shape, SPP_SUPPORTED_SHAPES};

use crate::swap::SwapError;

/// Outputs of a swap-side transfer: the payment to the recipient and the
/// sender's change (`ConfidentialTransaction::transfer` adds both; padding
/// fills the rest of the shape). The maker's fill rejects a user transfer
/// with any other count (`SwapError::UserTransferOutputs`).
pub const USER_OUTPUTS: usize = 2;

/// Picks the narrowest supported shape with at least `inputs` inputs and
/// `outputs` outputs, `None` if none fits.
pub fn smallest_shape(inputs: usize, outputs: usize) -> Option<Shape> {
    SPP_SUPPORTED_SHAPES
        .into_iter()
        .filter(|shape| shape.n_inputs() >= inputs && shape.n_outputs() >= outputs)
        .min_by_key(|shape| (shape.n_inputs(), shape.n_outputs()))
}

/// A confidential transfer of `amount` from `inputs` to `recipient`.
pub struct Transfer {
    /// UTXOs to spend, all of one asset; the first one's asset is sent.
    pub inputs: Vec<WalletUtxo>,
    /// Minimum proof shape input count; the shape uses the larger of this
    /// and `inputs.len()`.
    pub width: usize,
    pub amount: u64,
    pub recipient: ShieldedAddress,
    /// The fee payer named in the `transact` instruction.
    pub payer: Address,
    /// The state tree read for inputs and written for outputs.
    pub tree: Address,
    /// The zolana id of `tree`.
    pub tree_id: u16,
}

/// A proven transfer.
pub struct TransferInstruction {
    /// The zolana `transact` instruction, ready to include in a transaction.
    pub instruction: Instruction,
    /// Nullifiers of the real (non-padding) inputs, in input order.
    pub nullifiers: Vec<[u8; 32]>,
}

impl Transfer {
    /// Builds, encrypts and proves the transfer: the narrowest supported
    /// shape with at least `max(width, inputs.len())` inputs and
    /// `USER_OUTPUTS` outputs, padded with dummy UTXOs of the sender.
    ///
    /// Errors when `inputs` is empty, with `SwapError::NoSupportedShape`
    /// when no shape is wide enough, and with the transaction builder's or
    /// the prover's error.
    pub fn prove<R: Rpc>(
        self,
        client: &ZolanaClient<R>,
        keys: &dyn ShieldedKeys,
        authority: &dyn ProofAuthority,
    ) -> Result<TransferInstruction> {
        let Transfer {
            inputs,
            width,
            amount,
            recipient,
            payer,
            tree,
            tree_id,
        } = self;
        let asset = inputs
            .first()
            .map(|input| input.utxo.asset.asset)
            .ok_or_else(|| anyhow!("transfer without inputs"))?;
        let shape_inputs = width.max(inputs.len());
        let shape =
            smallest_shape(shape_inputs, USER_OUTPUTS).ok_or(SwapError::NoSupportedShape {
                inputs: shape_inputs,
                outputs: USER_OUTPUTS,
            })?;
        let identity = keys.address()?;
        let mut transaction =
            ConfidentialTransaction::new(inputs, payer)?.with_output_tree_id(tree_id)?;
        transaction.transfer(&recipient, asset, amount)?;
        transaction.pad_utxos(shape, &identity)?;
        let proof_inputs = transaction.encrypt(keys)?;
        let nullifiers = proof_inputs
            .input_utxos
            .iter()
            .filter(|input| !input.is_dummy())
            .map(|input| input.nullifier)
            .collect();
        let owner_signers = proof_inputs.owner_signer_pubkeys()?;
        let data = client
            .prove_transact(proof_inputs, None, authority)
            .map_err(|e| anyhow!("prove transfer: {e:?}"))?;
        let instruction = Transact {
            payer,
            input_trees: vec![tree],
            output_tree: tree,
            owner_signers,
            interface_transfer_accounts: Vec::new(),
            data,
        }
        .instruction();
        Ok(TransferInstruction {
            instruction,
            nullifiers,
        })
    }
}

/// Decrypts the outputs of a `transact` addressed to `keys`.
pub struct Receiver<'a> {
    pub keys: &'a dyn ShieldedKeys,
    /// Resolves the asset ids in a plaintext to mints.
    pub registry: &'a AssetRegistry,
    /// The output tree's zolana id, part of each output's commitment.
    pub tree_id: u16,
}

impl Receiver<'_> {
    /// The outputs of `data` encrypted to the receiver's viewing key, with
    /// their commitments, in output order. Outputs that are plaintext,
    /// another scheme or for another key are skipped.
    ///
    /// Every decrypted output is re-hashed and compared to the commitment
    /// the transaction publishes for it; a mismatch errors with
    /// `SwapError::CommitmentMismatch`, since the ciphertext alone does not
    /// bind the amount the chain records.
    pub fn received_outputs(&self, data: &TransactIxData) -> Result<Vec<(Utxo, [u8; 32])>> {
        let identity = self.keys.address()?;
        let mut received = Vec::new();
        for (slot, output) in data.outputs.iter().enumerate() {
            let Some(plaintext) =
                self.decrypt_output(&identity, output.data.as_deref(), data, slot)?
            else {
                continue;
            };
            let utxo = plaintext.into_utxo(identity.signing_pubkey, self.registry)?;
            let hash = utxo.hash(&identity.nullifier_pubkey, &[0; 32], &[0; 32], self.tree_id)?;
            if hash != output.utxo_hash {
                return Err(SwapError::CommitmentMismatch { slot }.into());
            }
            received.push((utxo, hash));
        }
        Ok(received)
    }

    fn decrypt_output(
        &self,
        identity: &ShieldedAddress,
        output: Option<&[u8]>,
        data: &TransactIxData,
        slot: usize,
    ) -> Result<Option<ConfidentialOutputPlaintext>> {
        let Some(output) = output else {
            return Ok(None);
        };
        let Ok(OutputDataEncoding::Encrypted(blob)) = OutputDataEncoding::try_from_slice(output)
        else {
            return Ok(None);
        };
        let Some((&scheme, body)) = blob.split_first() else {
            return Ok(None);
        };
        if scheme != EncryptedScheme::Confidential.as_byte()
            || Confidential::embedded_viewing_pk(body)? != identity.viewing_pubkey
        {
            return Ok(None);
        }
        let ciphertext = body
            .get(P256_PUBKEY_LEN..)
            .ok_or_else(|| anyhow!("output {slot} ciphertext is truncated"))?;
        let bytes = self
            .keys
            .decrypt(&[DecryptRequest {
                ciphertext,
                viewing_pubkey: identity.viewing_pubkey,
                tx_viewing_pubkey: P256Pubkey::from_bytes(data.tx_viewing_pk)?,
                salt: data.salt,
                slot_index: u32::try_from(slot)?,
                label: DecryptLabel::Utxo,
            }])?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("output {slot} decrypted to no plaintext"))?;
        Ok(Some(ConfidentialOutputPlaintext::deserialize(&bytes)?))
    }
}

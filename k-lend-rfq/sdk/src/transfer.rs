//! A confidential zolana transfer as either party builds it for a swap, and
//! the receiving side: decrypting the outputs addressed to a key and
//! checking each against its on-chain commitment before it is trusted.

use anyhow::{anyhow, Result};
use borsh::BorshDeserialize;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_client::{
    verify_confidential_transfer_inputs, ProofAuthority, ProofCompressed, Prover, ProverExt, Rpc,
    WitnessReader, ZolanaClient,
};
use zolana_event::OutputDataEncoding;
use zolana_interface::{instruction::TransactIxData, pda};
use zolana_keypair::{constants::P256_PUBKEY_LEN, P256Pubkey, ShieldedAddress};
use zolana_program::instruction::Transact;
use zolana_transaction::{
    instructions::transact::{ConfidentialTransaction, SppProofInputs},
    serialization::confidential::{Confidential, ConfidentialOutputPlaintext},
    AssetRegistry, DecryptLabel, DecryptRequest, EncryptedScheme, ShieldedKeys, Utxo, WalletUtxo,
};

use zolana_client::{Shape, SPP_SUPPORTED_SHAPES};

use crate::{
    address::{add_order_address, OrderAddress, WitnessRequest, ORDER_ADDRESS_TREE},
    swap::{OrderId, SwapError},
};

/// Outputs of a swap-side transfer: the payment to the recipient and the
/// sender's change (`ConfidentialTransaction::transfer` adds both; padding
/// fills the rest of the shape). The market maker's fill rejects a user
/// transfer with any other count (`SwapError::UserTransferOutputs`).
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
        let (proof_inputs, nullifiers) = self.encrypt(keys)?;
        let owner_signers = proof_inputs.owner_signer_pubkeys()?;
        let ix_data = client
            .prove_transact(proof_inputs, None, authority)
            .map_err(|error| anyhow!("prove transfer: {error:?}"))?;
        Ok(self.finish(vec![self.tree], owner_signers, ix_data, nullifiers))
    }

    /// Like [`Self::prove`], with the address slot of order `order`, owned by
    /// `payer`, in the input slot after the real inputs: the market maker's
    /// fill transfer, which records the order as filled (`crate::address`).
    /// `width` must leave the slot room: `inputs.len() +
    /// ORDER_ADDRESS_SLOTS` pads to a shape with a spare input.
    ///
    /// Witnesses come from `client`'s indexer, the address's non-inclusion
    /// proof from the nullifier tree of `ORDER_ADDRESS_TREE`, and `prover`
    /// proves the hand-assembled witness (`add_order_address`); the proof is
    /// verified locally before the instruction is built.
    ///
    /// Errors like [`Self::prove`], with `SwapError::OrderAddressDerivation`
    /// and with `add_order_address`'s errors.
    pub fn prove_with_order_address<R: Rpc>(
        self,
        client: &ZolanaClient<R>,
        prover: &dyn Prover,
        keys: &dyn ShieldedKeys,
        authority: &dyn ProofAuthority,
        order: OrderId,
    ) -> Result<TransferInstruction> {
        let order = OrderAddress::new(&self.payer, order)?;
        let (proof_inputs, nullifiers) = self.encrypt(keys)?;
        let owner_signers = proof_inputs.owner_signer_pubkeys()?;
        let indexer = client.indexer();
        let request = WitnessRequest::new(&proof_inputs, self.tree_id, Some(&order));
        let mut witnesses = indexer.input_witnesses(
            &proof_inputs.input_utxo_hashes()?,
            &request.dummy_nullifiers,
            None,
        )?;
        let separate = request
            .separate_address()
            .map(|leaf| {
                indexer.get_non_inclusion_proofs(pda::tree(ORDER_ADDRESS_TREE), vec![leaf], None)
            })
            .transpose()?
            .and_then(|response| response.proofs.into_iter().next());
        let non_inclusion =
            request.address_proof(&mut witnesses.dummy_nullifier_proofs, separate)?;
        let (mut assembled, patch) = add_order_address(
            proof_inputs,
            &witnesses.spend_proofs,
            &witnesses.dummy_nullifier_proofs,
            &order,
            &non_inclusion,
        )?;
        authority.complete_inputs(&mut assembled.prover_inputs.inputs)?;
        let proof = prover.prove_transfer(&assembled.prover_inputs)?;
        verify_confidential_transfer_inputs(
            &assembled.prover_inputs,
            assembled.public_input_hash,
            &proof,
        )?;
        let input_trees = assembled
            .input_tree_ids
            .iter()
            .copied()
            .map(pda::tree)
            .collect();
        let mut data = assembled.with_proof(ProofCompressed::try_from(proof)?.to_transact_proof());
        patch.apply(&mut data);
        Ok(self.finish(input_trees, owner_signers, data, nullifiers))
    }

    /// The encrypted transfer padded to the narrowest shape with
    /// `max(width, inputs.len())` inputs, and the nullifiers of its real
    /// inputs.
    fn encrypt(&self, keys: &dyn ShieldedKeys) -> Result<(SppProofInputs, Vec<[u8; 32]>)> {
        let asset = self
            .inputs
            .first()
            .map(|input| input.utxo.asset.asset)
            .ok_or_else(|| anyhow!("transfer without inputs"))?;
        let shape_inputs = self.width.max(self.inputs.len());
        let shape =
            smallest_shape(shape_inputs, USER_OUTPUTS).ok_or(SwapError::NoSupportedShape {
                inputs: shape_inputs,
                outputs: USER_OUTPUTS,
            })?;
        let identity = keys.address()?;
        let mut transaction = ConfidentialTransaction::new(self.inputs.clone(), self.payer)?
            .with_output_tree_id(self.tree_id)?;
        transaction.transfer(&self.recipient, asset, self.amount)?;
        transaction.pad_utxos(shape, &identity)?;
        let proof_inputs = transaction.encrypt(keys)?;
        let nullifiers = proof_inputs
            .input_utxos
            .iter()
            .filter(|input| !input.is_dummy())
            .map(|input| input.nullifier)
            .collect();
        Ok((proof_inputs, nullifiers))
    }

    /// The `transact` of this transfer paid by `payer` with outputs in
    /// `tree`, naming `input_trees` and carrying `data`.
    fn finish(
        &self,
        input_trees: Vec<Address>,
        owner_signers: Vec<Address>,
        data: TransactIxData,
        nullifiers: Vec<[u8; 32]>,
    ) -> TransferInstruction {
        let instruction = Transact {
            payer: self.payer,
            input_trees,
            output_tree: self.tree,
            owner_signers,
            interface_transfer_accounts: Vec::new(),
            data,
        }
        .instruction();
        TransferInstruction {
            instruction,
            nullifiers,
        }
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
    /// The outputs of `transact` encrypted to the receiver's viewing key, with
    /// their commitments, in output order. Outputs that are plaintext,
    /// another scheme or for another key are skipped.
    ///
    /// Every decrypted output is re-hashed and compared to the commitment
    /// the transaction publishes for it; a mismatch errors with
    /// `SwapError::CommitmentMismatch`, since the ciphertext alone does not
    /// bind the amount the chain records.
    pub fn received_outputs(&self, transact: &TransactIxData) -> Result<Vec<(Utxo, [u8; 32])>> {
        let identity = self.keys.address()?;
        let mut received = Vec::new();
        for (slot, output) in transact.outputs.iter().enumerate() {
            let Some(plaintext) =
                self.decrypt_output(&identity, output.data.as_deref(), transact, slot)?
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

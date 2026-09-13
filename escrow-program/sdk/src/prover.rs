use anyhow::{ensure, Result};
use timelock_escrow_prover::{EscrowProofInputs, TimelockProof, WithdrawProofInputs};
use zolana_client::{
    into_prover, NonInclusionProof, ProofCompressed, ProverClient, ProverVariant, SpendProof,
};
use zolana_hasher::primitives::hash_bytes;
use zolana_interface::{
    instruction::instruction_data::transact::{CircuitId, InputUtxo, TransactIxData},
    N_PUBLIC_SLOTS,
};
use zolana_transaction::instructions::transact::SppProofInputs;

use crate::{err, escrow_authority_pda};

#[derive(Default)]
pub struct EscrowProverClient;

impl EscrowProverClient {
    pub fn new() -> Self {
        Self
    }

    /// Prove the creator-funded 2x2 escrow transaction. Fetch both real-input
    /// and dummy nullifier witnesses from the builder's `input_tree` first.
    /// The PDA authorizes the data-bearing output through signed CPI even
    /// though the source input belongs to the creator, who is also the payer.
    pub fn prove_spp_escrow(
        &self,
        prover: &ProverClient,
        proof_inputs: SppProofInputs,
        spend_proofs: &[SpendProof],
        dummy_nullifier_proofs: &[NonInclusionProof],
    ) -> Result<TransactIxData> {
        let circuit = Self::escrow_spp_prover(proof_inputs, spend_proofs, dummy_nullifier_proofs)?;
        let external = circuit.external_data.clone();
        let result = circuit.build().map_err(err)?;
        let proof = prover.prove_transfer(&result.inputs).map_err(err)?;
        let proof = ProofCompressed::try_from(proof)
            .map_err(err)?
            .to_transact_proof();
        ensure!(
            result.nullifiers.len() == 2 && result.input_root_indices.len() == 2,
            "escrow prover returned an incompatible input shape"
        );
        let inputs = result
            .nullifiers
            .into_iter()
            .zip(result.input_root_indices)
            .map(
                |(nullifier_hash, (utxo_tree_root_index, nullifier_tree_root_index))| InputUtxo {
                    nullifier_hash,
                    utxo_tree_root_index,
                    nullifier_tree_root_index,
                },
            )
            .collect();
        Ok(TransactIxData {
            proof,
            expiry_unix_ts: external.expiry_unix_ts,
            private_tx_hash: result.private_tx_hash,
            circuit: CircuitId::ConfidentialEddsa(2, 2, N_PUBLIC_SLOTS as u8),
            inputs,
            interface_transfers: vec![],
            data_hash: external.data_hash,
            ring_data_hash: external.ring_data_hash,
            tx_viewing_pk: external.tx_viewing_pk,
            salt: external.salt,
            outputs: external.outputs,
            messages: external.messages,
        })
    }

    fn escrow_spp_prover(
        proof_inputs: SppProofInputs,
        spend_proofs: &[SpendProof],
        dummy_nullifier_proofs: &[NonInclusionProof],
    ) -> Result<zolana_client::TransferProver> {
        ensure!(
            proof_inputs.input_utxos.len() == 2 && proof_inputs.output_utxos.len() == 2,
            "escrow requires two inputs and two outputs"
        );
        let source = &proof_inputs.input_utxos[0];
        ensure!(
            !source.is_dummy() && proof_inputs.input_utxos[1].is_dummy(),
            "escrow requires a real source input followed by a dummy"
        );
        ensure!(
            source.utxo.owner.as_ed25519().map_err(err)? == proof_inputs.payer.to_bytes(),
            "escrow source owner must be the payer"
        );
        ensure!(
            proof_inputs.external_data.interface_transfers.is_empty(),
            "escrow does not support public settlement transfers"
        );
        ensure!(
            spend_proofs.len() == 1 && dummy_nullifier_proofs.len() == 1,
            "escrow requires one real-input proof and one dummy nullifier proof"
        );
        let payer_hash = hash_bytes(proof_inputs.payer.as_array()).map_err(err)?;
        let authority_hash = hash_bytes(escrow_authority_pda().as_array()).map_err(err)?;
        let built = into_prover(proof_inputs, spend_proofs, dummy_nullifier_proofs).map_err(err)?;
        let ProverVariant::Eddsa(mut circuit) = built.circuit;
        // This exact payer/PDA order is also reconstructed by SPP from the CPI
        // accounts. Set it before building either the witness or public hash.
        circuit.signer_pk_hashes = vec![payer_hash, authority_hash, [0u8; 32]];
        Ok(circuit)
    }

    pub fn prove_escrow(&self, inputs: &EscrowProofInputs) -> Result<TimelockProof> {
        inputs.prove().map_err(err)
    }

    pub fn prove_withdraw(&self, inputs: &WithdrawProofInputs) -> Result<TimelockProof> {
        inputs.prove().map_err(err)
    }
}

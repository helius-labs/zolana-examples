use anyhow::{ensure, Result};
use solana_address::Address;
use zolana_client::{
    prover::transact::{attach_input_proofs, input_utxos_from_nullifiers, TransferProver},
    verify_confidential_transfer_inputs, NonInclusionProof, ProofAuthority, ProofCompressed,
    Prover, ProverExt, SpendProof,
};
use zolana_hasher::primitives::solana_owner_identity;
use zolana_interface::{
    instruction::instruction_data::transact::{CircuitId, TransactIxData},
    pda, N_PUBLIC_SLOTS,
};
use zolana_transaction::instructions::transact::SppProofInputs;

pub struct SppProofRequest {
    pub inputs: SppProofInputs,
    pub spend_proofs: Vec<SpendProof>,
    pub padding_proofs: Vec<NonInclusionProof>,
    /// PDA owners authorizing program data outputs, beyond the funding owners.
    pub additional_signers: Vec<Address>,
}

pub struct SppTransaction {
    pub data: TransactIxData,
    pub input_trees: Vec<Address>,
    pub owner_signers: Vec<Address>,
}

impl SppProofRequest {
    pub fn prove(
        self,
        prover: &impl Prover,
        authority: &dyn ProofAuthority,
    ) -> Result<SppTransaction> {
        let Self {
            inputs,
            spend_proofs,
            padding_proofs,
            additional_signers,
        } = self;
        let shape = inputs.check_shape()?;
        let mut owner_signers = inputs.owner_signer_pubkeys()?;
        for signer in additional_signers {
            if signer != inputs.payer && !owner_signers.contains(&signer) {
                owner_signers.push(signer);
            }
        }
        let mut signer_pk_hashes = vec![solana_owner_identity(inputs.payer.as_array())?];
        for signer in &owner_signers {
            signer_pk_hashes.push(solana_owner_identity(signer.as_array())?);
        }
        ensure!(
            signer_pk_hashes.len() <= shape.signer_width(),
            "too many owner signers for SPP shape"
        );
        signer_pk_hashes.resize(shape.signer_width(), [0; 32]);
        let external = inputs.external_data.clone();
        let public_transfers = inputs.public_transfers()?;
        let mut result = TransferProver {
            inputs: attach_input_proofs(inputs.input_utxos, &spend_proofs, &padding_proofs)?,
            outputs: inputs.output_utxos,
            blinding_seed: inputs.blinding_seed,
            output_tree_id: inputs.output_tree_id,
            external_data: inputs.external_data,
            public_transfers,
            signer_pk_hashes,
            allow_dummy_inputs: true,
            shape,
            cache_accounts: inputs.cache_accounts,
        }
        .build()?;
        ensure!(
            result.cache_access.is_none(),
            "staking SPP transactions do not use cached inputs"
        );
        authority.complete_inputs(&mut result.inputs.inputs)?;
        let proof = prover.prove_transfer(&result.inputs)?;
        verify_confidential_transfer_inputs(&result.inputs, result.public_input_hash, &proof)?;
        let data = TransactIxData {
            expiry_unix_ts: external.expiry_unix_ts,
            tx_viewing_pk: external.tx_viewing_pk,
            salt: external.salt,
            interface_transfers: external
                .interface_transfers
                .iter()
                .map(|transfer| transfer.interface_transfer())
                .collect(),
            data_hash: external.data_hash,
            ring_data_hash: external.ring_data_hash,
            outputs: external.outputs,
            messages: external.messages,
            private_tx_hash: result.private_tx_hash,
            circuit: CircuitId::ConfidentialEddsa(
                shape.n_inputs() as u8,
                shape.n_outputs() as u8,
                N_PUBLIC_SLOTS as u8,
            ),
            proof: ProofCompressed::try_from(proof)?.to_transact_proof(),
            inputs: input_utxos_from_nullifiers(&result.nullifiers, &result.input_tree_indexes)?,
            tree_contexts: result.tree_contexts,
        };
        Ok(SppTransaction {
            data,
            input_trees: result.tree_ids.into_iter().map(pda::tree).collect(),
            owner_signers,
        })
    }
}

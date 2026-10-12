use anyhow::{ensure, Result};
use solana_address::Address;
use zolana_client::{
    prover::transact::{attach_input_proofs, input_utxos_from_nullifiers, TransferProver},
    verify_confidential_transfer_inputs, NonInclusionProof, ProofAuthority, ProofCompressed,
    Prover, ProverExt, SpendProof, TransferInputs,
};
use zolana_hasher::primitives::solana_owner_identity;
use zolana_interface::{
    instruction::instruction_data::transact::{
        CacheAccess, CircuitId, TransactIxData, TransactProof,
    },
    pda, N_PUBLIC_SLOTS,
};
use zolana_transaction::instructions::transact::SppProofInputs;

pub struct SppProofRequest {
    pub inputs: SppProofInputs,
    pub spend_proofs: Vec<SpendProof>,
    pub padding_proofs: Vec<NonInclusionProof>,
    /// PDA owners authorizing program data outputs, beyond the funding owners.
    pub additional_signers: Vec<Address>,
    /// Published dummy-input policy; SPP derives it from the input trees'
    /// headroom, which is `true` for every tree with spare capacity.
    pub allow_dummy_inputs: bool,
    /// The cache access the transaction must prove: `None` for plain
    /// transfers, the settlement legs' fixed access otherwise.
    pub expected_cache_access: Option<CacheAccess>,
}

pub struct SppTransaction {
    pub data: TransactIxData,
    pub input_trees: Vec<Address>,
    pub owner_signers: Vec<Address>,
    pub cache_access: Option<CacheAccess>,
}

impl SppProofRequest {
    /// An uncached request with dummy inputs allowed.
    pub fn new(
        inputs: SppProofInputs,
        spend_proofs: Vec<SpendProof>,
        padding_proofs: Vec<NonInclusionProof>,
        additional_signers: Vec<Address>,
    ) -> Self {
        Self {
            inputs,
            spend_proofs,
            padding_proofs,
            additional_signers,
            allow_dummy_inputs: true,
            expected_cache_access: None,
        }
    }

    pub fn prove(
        self,
        prover: &impl Prover,
        authority: &dyn ProofAuthority,
    ) -> Result<SppTransaction> {
        self.assemble()?.prove(prover, authority)
    }

    /// Builds the witness and the instruction data (with a zeroed proof)
    /// exactly as [`prove`](Self::prove) does, without proving.
    pub fn assemble(self) -> Result<AssembledSpp> {
        let Self {
            inputs,
            spend_proofs,
            padding_proofs,
            additional_signers,
            allow_dummy_inputs,
            expected_cache_access,
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
        let result = TransferProver {
            inputs: attach_input_proofs(inputs.input_utxos, &spend_proofs, &padding_proofs)?,
            outputs: inputs.output_utxos,
            blinding_seed: inputs.blinding_seed,
            output_tree_id: inputs.output_tree_id,
            external_data: inputs.external_data,
            public_transfers,
            signer_pk_hashes,
            allow_dummy_inputs,
            shape,
            cache_accounts: inputs.cache_accounts,
        }
        .build()?;
        ensure!(
            result.cache_access == expected_cache_access,
            "unexpected SPP cache access"
        );
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
            circuit: circuit_id(shape.n_inputs(), shape.n_outputs(), result.cache_access)?,
            proof: TransactProof::zeroed(),
            inputs: input_utxos_from_nullifiers(&result.nullifiers, &result.input_tree_indexes)?,
            tree_contexts: result.tree_contexts,
        };
        Ok(AssembledSpp {
            witness: result.inputs,
            public_input_hash: result.public_input_hash,
            transaction: SppTransaction {
                data,
                input_trees: result.tree_ids.into_iter().map(pda::tree).collect(),
                owner_signers,
                cache_access: result.cache_access,
            },
        })
    }
}

/// An assembled SPP transaction awaiting its proof.
pub struct AssembledSpp {
    witness: TransferInputs,
    public_input_hash: [u8; 32],
    /// The transaction with a zeroed proof.
    pub transaction: SppTransaction,
}

impl AssembledSpp {
    pub fn prove(
        mut self,
        prover: &impl Prover,
        authority: &dyn ProofAuthority,
    ) -> Result<SppTransaction> {
        authority.complete_inputs(&mut self.witness.inputs)?;
        let proof = prover.prove_transfer(&self.witness)?;
        verify_confidential_transfer_inputs(&self.witness, self.public_input_hash, &proof)?;
        self.transaction.data.proof = ProofCompressed::try_from(proof)?.to_transact_proof();
        Ok(self.transaction)
    }
}

/// Mirrors zolana-client's `assemble_with_dummy_policy` circuit selection.
fn circuit_id(n_in: usize, n_out: usize, access: Option<CacheAccess>) -> Result<CircuitId> {
    let n_in = u8::try_from(n_in)?;
    let n_out = u8::try_from(n_out)?;
    let slots = u8::try_from(N_PUBLIC_SLOTS)?;
    Ok(match access {
        Some(access) => CircuitId::ConfidentialEddsaCached(n_in, n_out, slots, access),
        None => CircuitId::ConfidentialEddsa(n_in, n_out, slots),
    })
}

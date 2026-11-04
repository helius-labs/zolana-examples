use anyhow::{ensure, Result};
use dvp_program::circuits::CreateDvp as CreateDvpInputs;
use solana_address::Address;
use zolana_client::{NonInclusionProof, ProverExt};
use zolana_keypair::{ShieldedAddress, ViewingKey};
use zolana_program::{ProgramOwner, ZkProgram};
use zolana_transaction::instructions::transact::SppProofInputs;

use super::{proof::create_inputs, CreateDvp, CreateDvpDerivation};
use crate::{
    encryption::{encrypt_terms_messages, TERMS_RECIPIENTS},
    err,
    state::SwapDvp,
};

pub struct CreateDvpTransactionParams {
    pub terms: SwapDvp,
    pub payer: Address,
    pub recipients: [ShieldedAddress; TERMS_RECIPIENTS],
    pub tree_id: u16,
    pub non_inclusion: NonInclusionProof,
}

pub struct CreateDvpTransaction {
    payer: Address,
    creator: Address,
    non_inclusion: NonInclusionProof,
    pub derivation: CreateDvpDerivation,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: CreateDvpInputs,
}

impl CreateDvpTransactionParams {
    /// 1. Check the address tree and non-inclusion proof.
    /// 2. Build the program transaction.
    /// 3. Encrypt the terms for all three parties.
    pub fn build(self) -> Result<CreateDvpTransaction> {
        // 1. Check the address tree and non-inclusion proof.
        ensure!(
            self.tree_id == dvp_program::DVP_ADDRESS_TREE_ID,
            "the DvP address uses tree {}",
            dvp_program::DVP_ADDRESS_TREE_ID
        );
        let derivation = CreateDvpDerivation::new(&self.terms, self.tree_id)?;
        ensure!(
            self.non_inclusion.leaf == derivation.address,
            "the non-inclusion proof is not for the DvP address"
        );
        // 2. Build the program transaction.
        let viewing_key = ViewingKey::new();
        let owner = ProgramOwner::new(derivation.dvp_authority)
            .address(viewing_key.pubkey())
            .map_err(err)?;
        let proof_inputs = create_inputs(&self.terms, owner, self.tree_id)?;
        let finalized = proof_inputs
            .create_finalized_transaction(&owner, self.payer)
            .map_err(err)?;
        let mut spp_proof_inputs = finalized
            .encrypt_with_viewing_key(&viewing_key)
            .map_err(err)?;
        // 3. Encrypt the terms for all three parties.
        for output in &mut spp_proof_inputs.external_data.outputs {
            output.data = None;
        }
        spp_proof_inputs.external_data.messages = encrypt_terms_messages(
            &self.terms,
            &self.recipients,
            &viewing_key,
            spp_proof_inputs.external_data.salt,
        )?;
        Ok(CreateDvpTransaction {
            payer: self.payer,
            creator: self.terms.creator,
            non_inclusion: self.non_inclusion,
            derivation,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl CreateDvpTransaction {
    /// 1. Prove the SPP transaction.
    /// 2. Prove create_dvp and build the instruction.
    pub fn prove(self, prover: &impl ProverExt, tree: Address) -> Result<CreateDvp> {
        // 1. Prove the SPP transaction.
        let spp = prover
            .prove_transact(
                self.spp_proof_inputs,
                &[],
                &[self.non_inclusion],
                &ProgramOwner::nullifier_key(),
            )
            .map_err(err)?;
        ensure!(
            spp.private_tx_hash == self.derivation.private_tx_hash,
            "the SPP and create proofs cover different transactions"
        );
        // 2. Prove create_dvp and build the instruction.
        let create_proof =
            crate::proof::prove(crate::proof::CircuitId::CreateDvp, &self.proof_inputs)?;
        Ok(CreateDvp {
            payer: self.payer,
            creator: self.creator,
            tree,
            dvp_id: self.derivation.dvp_id,
            bump: self.derivation.bump,
            create_proof,
            spp,
        })
    }
}

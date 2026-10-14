use anyhow::Result;
use dvp_program::{
    instructions::{create_dvp::CreateDvpPublicInput, shared::dvp_authority_owner_hash},
    DVP_ADDRESS_TREE_ID,
};
use dvp_prover::{CreateDvpProofInputs, ProofInputUtxo, SwapDvpProofInput};
use zolana_hasher::primitives::solana_owner_identity;
use zolana_program::{
    compression::{AddressSeed, NewAddress, PdaOwner},
    derive_output_blinding_seed, derive_private_tx_blinding, derive_transact_output_blinding,
    PrivateTxHash,
};
use zolana_transaction::SOL_MINT;

use crate::{err, state::SwapDvp};

/// Everything the create proof needs: the terms and the tree the DvP UTXO is
/// appended to. Every other witness value is derived from them, the same way
/// the SPP transaction the instruction CPIs derives it.
pub struct CreateDvpProofInputParams {
    pub terms: SwapDvp,
    /// Raw id of the output tree; it is the second element of the DvP UTXO's
    /// commitment.
    pub dvp_utxo_tree_id: u16,
}

impl CreateDvpProofInputParams {
    /// Validates the terms, then derives:
    /// - `dvp_id`, the DvP authority PDA and its owner hash;
    /// - the address the 1x2 transaction's address slot creates (seed `dvp_id`,
    ///   tree `DVP_ADDRESS_TREE_ID`, D6), which is its first nullifier;
    /// - the DvP UTXO with output-0 blinding and the private tx blinding, both
    ///   from the create seed `Poseidon(CREATE_TAG, terms.blinding)`;
    /// - `private_tx_hash` over `[0]` inputs, `[DvP UTXO, 0]` outputs and
    ///   `[address]`, and the public input hash.
    pub fn to_proof_inputs(&self) -> Result<CreateDvpProofInputs> {
        let terms = &self.terms;
        terms.validate()?;
        let terms_input = SwapDvpProofInput::try_from(terms)?;
        let dvp_id = terms.dvp_id()?;
        let (dvp_authority, _) = terms.dvp_authority()?;
        let owner_hash = dvp_authority_owner_hash(&dvp_authority).map_err(err)?;
        let address = NewAddress::derive(
            &PdaOwner::new(&dvp_authority).map_err(err)?,
            AddressSeed::new(dvp_id).map_err(err)?,
            DVP_ADDRESS_TREE_ID,
        )
        .map_err(err)?;
        let first_nullifier = address.address();

        let blinding_seed = terms.create_blinding_seed()?;
        let output_blinding_seed =
            derive_output_blinding_seed(first_nullifier, &blinding_seed).map_err(err)?;
        let dvp_utxo_blinding =
            derive_transact_output_blinding(first_nullifier, &output_blinding_seed, 0)
                .map_err(err)?;
        let private_tx_blinding =
            derive_private_tx_blinding(first_nullifier, &blinding_seed).map_err(err)?;

        let dvp_utxo = ProofInputUtxo::new(
            owner_hash,
            &SOL_MINT,
            0,
            &dvp_utxo_blinding,
            self.dvp_utxo_tree_id,
        )
        .map_err(err)?
        .with_data_hash(dvp_id);
        let private_tx_hash = PrivateTxHash {
            input_hashes: &[[0u8; 32]],
            output_hashes: &[dvp_utxo.hash().map_err(err)?, [0u8; 32]],
            address_nullifiers: Some(&[*first_nullifier]),
            blinding: &private_tx_blinding,
        }
        .hash()
        .map_err(err)?;

        let creator_id = solana_owner_identity(terms.creator.as_array()).map_err(err)?;
        let public_input_hash = CreateDvpPublicInput {
            private_tx_hash: &private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &owner_hash,
        }
        .hash()
        .map_err(err)?;

        Ok(CreateDvpProofInputs {
            public_input_hash,
            private_tx_hash,
            creator_id,
            dvp_id,
            dvp_authority_owner_hash: owner_hash,
            terms: terms_input,
            dvp_utxo,
            private_tx_blinding,
        })
    }
}

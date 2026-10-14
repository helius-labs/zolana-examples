use anyhow::{ensure, Result};
use dvp_program::{
    instructions::{create_dvp::CreateDvpPublicInput, shared::dvp_authority_owner_hash},
    DVP_ADDRESS_TREE_ID,
};
use dvp_prover::{CreateDvpProofInputs, ProofInputUtxo, SwapDvpProofInput};
use solana_address::Address;
use zolana_hasher::primitives::{right_align, solana_owner_identity};
use zolana_interface::{tree_slot::tree_id_field, ADDRESS_DOMAIN};
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

/// The values the create proof, the create SPP transaction and every later
/// reader of the DvP UTXO derive from the terms and the output tree, computed
/// once so they cannot drift apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateDvpDerivation {
    pub terms: SwapDvpProofInput,
    pub creator_id: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_authority: Address,
    pub bump: u8,
    /// `Poseidon(id(dvp_authority), ZERO_NULLIFIER_PUBKEY)`: owner of the
    /// address slot and the DvP UTXO.
    pub owner_hash: [u8; 32],
    /// The address slot `{AddressDomain, DVP_ADDRESS_TREE_ID, owner_hash,
    /// blinding dvp_id}` the 1x2 transaction spends in input slot 0.
    pub address_slot: ProofInputUtxo,
    /// `Poseidon(address_slot, dvp_id, 0)`: the slot's nullifier, and the
    /// transaction's first nullifier.
    pub address: [u8; 32],
    /// `Poseidon(CREATE_TAG, terms.blinding)`.
    pub blinding_seed: [u8; 32],
    /// The DvP UTXO (output 0), with the output-0 blinding.
    pub dvp_utxo: ProofInputUtxo,
    pub dvp_utxo_hash: [u8; 32],
    /// Blinding of the compact padding output in slot 1. SPP checks every
    /// output blinding against the derivation, compact padding included.
    pub padding_blinding: [u8; 32],
    pub private_tx_blinding: [u8; 32],
    /// Over `[0]` inputs, `[DvP UTXO, 0]` outputs and `[address]`.
    pub private_tx_hash: [u8; 32],
}

impl CreateDvpDerivation {
    /// Validates the terms, then derives:
    /// - `dvp_id`, the DvP authority PDA with its bump, and its owner hash;
    /// - the address the 1x2 transaction's address slot creates (seed `dvp_id`,
    ///   tree `DVP_ADDRESS_TREE_ID`), which is its first nullifier;
    /// - both output blindings and the private tx blinding, from the create
    ///   seed `Poseidon(CREATE_TAG, terms.blinding)` and that address;
    /// - the DvP UTXO and `private_tx_hash`.
    pub fn new(terms: &SwapDvp, dvp_utxo_tree_id: u16) -> Result<Self> {
        terms.validate()?;
        let terms_input = SwapDvpProofInput::try_from(terms)?;
        let creator_id = solana_owner_identity(terms.creator.as_array()).map_err(err)?;
        let dvp_id = terms.dvp_id()?;
        let (dvp_authority, bump) = terms.dvp_authority()?;
        let owner_hash = dvp_authority_owner_hash(&dvp_authority).map_err(err)?;
        let new_address = NewAddress::derive(
            &PdaOwner::new(&dvp_authority).map_err(err)?,
            AddressSeed::new(dvp_id).map_err(err)?,
            DVP_ADDRESS_TREE_ID,
        )
        .map_err(err)?;
        let address_slot = ProofInputUtxo {
            domain: right_align(&ADDRESS_DOMAIN.to_be_bytes()),
            tree_id: tree_id_field(DVP_ADDRESS_TREE_ID),
            owner_hash,
            blinding: dvp_id,
            ..ProofInputUtxo::default()
        };
        ensure!(
            address_slot.hash().map_err(err)? == *new_address.utxo_hash(),
            "address slot does not hash to the derived address utxo"
        );
        let address = *new_address.address();

        let blinding_seed = terms.create_blinding_seed()?;
        let output_blinding_seed =
            derive_output_blinding_seed(&address, &blinding_seed).map_err(err)?;
        let dvp_utxo_blinding =
            derive_transact_output_blinding(&address, &output_blinding_seed, 0).map_err(err)?;
        let padding_blinding =
            derive_transact_output_blinding(&address, &output_blinding_seed, 1).map_err(err)?;
        let private_tx_blinding =
            derive_private_tx_blinding(&address, &blinding_seed).map_err(err)?;

        let dvp_utxo = ProofInputUtxo::new(
            owner_hash,
            &SOL_MINT,
            0,
            &dvp_utxo_blinding,
            dvp_utxo_tree_id,
        )
        .map_err(err)?
        .with_data_hash(dvp_id);
        let dvp_utxo_hash = dvp_utxo.hash().map_err(err)?;
        let private_tx_hash = PrivateTxHash {
            input_hashes: &[[0u8; 32]],
            output_hashes: &[dvp_utxo_hash, [0u8; 32]],
            address_nullifiers: Some(&[address]),
            blinding: &private_tx_blinding,
        }
        .hash()
        .map_err(err)?;

        Ok(Self {
            terms: terms_input,
            creator_id,
            dvp_id,
            dvp_authority,
            bump,
            owner_hash,
            address_slot,
            address,
            blinding_seed,
            dvp_utxo,
            dvp_utxo_hash,
            padding_blinding,
            private_tx_blinding,
            private_tx_hash,
        })
    }

    /// The derived values plus the public input hash over them.
    pub fn to_proof_inputs(&self) -> Result<CreateDvpProofInputs> {
        let public_input_hash = CreateDvpPublicInput {
            private_tx_hash: &self.private_tx_hash,
            creator_id: &self.creator_id,
            dvp_id: &self.dvp_id,
            dvp_authority_owner_hash: &self.owner_hash,
        }
        .hash()
        .map_err(err)?;

        Ok(CreateDvpProofInputs {
            public_input_hash,
            private_tx_hash: self.private_tx_hash,
            creator_id: self.creator_id,
            dvp_id: self.dvp_id,
            dvp_authority_owner_hash: self.owner_hash,
            terms: self.terms,
            dvp_utxo: self.dvp_utxo.clone(),
            private_tx_blinding: self.private_tx_blinding,
        })
    }
}

impl CreateDvpProofInputParams {
    /// The create witness of the [`CreateDvpDerivation`] of the terms.
    pub fn to_proof_inputs(&self) -> Result<CreateDvpProofInputs> {
        CreateDvpDerivation::new(&self.terms, self.dvp_utxo_tree_id)?.to_proof_inputs()
    }
}

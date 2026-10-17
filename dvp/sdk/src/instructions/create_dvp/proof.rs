use anyhow::{ensure, Result};
use dvp_program::circuits::{
    CreateDvp, CreateDvpPrivateInputs, CreateDvpPublicInputs, SwapDvpTerms,
};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::ViewingKey;
use zolana_program::{
    compression::{AddressSeed, NewAddress, PdaOwner},
    ProgramOwner, TxContext, ZkProgram,
};
use zolana_transaction::SppProofOutputUtxo;

use crate::{err, state::SwapDvp};

#[derive(Clone, Debug)]
pub struct CreateDvpDerivation {
    pub dvp_id: [u8; 32],
    pub dvp_authority: Address,
    pub bump: u8,
    pub address: [u8; 32],
    pub dvp_utxo: SppProofOutputUtxo,
    pub dvp_utxo_hash: [u8; 32],
    pub private_tx_hash: [u8; 32],
}

impl CreateDvpDerivation {
    /// 1. Check terms and reserve the DvP address.
    /// 2. Derive the state UTXO through the program circuit.
    pub fn new(terms: &SwapDvp, tree_id: u16) -> Result<Self> {
        // 1. Check terms and reserve the DvP address.
        terms.validate()?;
        let dvp_id = terms.dvp_id()?;
        let (dvp_authority, bump) = terms.dvp_authority()?;
        let address = *NewAddress::derive_with_tree_id(
            &PdaOwner::new(&dvp_authority).map_err(err)?,
            AddressSeed::new(dvp_id).map_err(err)?,
            dvp_program::DVP_ADDRESS_TREE_ID,
        )
        .map_err(err)?
        .address();
        // 2. Derive the state UTXO through the program circuit.
        let owner = ProgramOwner::new(dvp_authority)
            .address(ViewingKey::new().pubkey())
            .map_err(err)?;
        let inputs = create_inputs(terms, owner, tree_id)?;
        let finalized = inputs
            .create_finalized_transaction(&owner, terms.creator)
            .map_err(err)?;
        let output = finalized
            .output_utxos()
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing DvP output"))?;
        let dvp_utxo = output.clone();
        let dvp_utxo_hash = dvp_utxo.hash(tree_id).map_err(err)?;
        ensure!(
            output.data_hash == Some(dvp_id),
            "the DvP output does not commit to the terms"
        );
        Ok(Self {
            dvp_id,
            dvp_authority,
            bump,
            address,
            dvp_utxo,
            dvp_utxo_hash,
            private_tx_hash: finalized
                .padding_independent_private_tx_hash()
                .map_err(err)?,
        })
    }
}

pub(crate) fn create_inputs(
    terms: &SwapDvp,
    owner: zolana_keypair::ShieldedAddress,
    tree_id: u16,
) -> Result<CreateDvp> {
    Ok(CreateDvp {
        private: CreateDvpPrivateInputs {
            tx_context: TxContext::new()
                .with_blinding_seed(terms.create_blinding_seed()?)
                .with_output_tree_id(Some(tree_id)),
            terms: SwapDvpTerms::try_from(terms)?,
        },
        public: CreateDvpPublicInputs {
            creator_identity: solana_owner_identity(terms.creator.as_array()).map_err(err)?,
            dvp_id: terms.dvp_id()?,
            dvp_owner: owner,
        },
    })
}

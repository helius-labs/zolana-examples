use anyhow::Result;
use dvp_program::instructions::shared::dvp_authority;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use zolana_interface::{
    instruction::instruction_data::transact::TransactIxData, SHIELDED_POOL_PROGRAM_ID,
};
use zolana_program::instruction::nullifier_pda_accounts;

use crate::{err, tag, CreateDvpIxData, CreateDvpProof};

pub struct CreateDvp {
    pub payer: Pubkey,
    /// The terms' creator, a readonly signer whose identity the create proof
    /// binds. It also seeds `dvp_authority`.
    pub creator: Pubkey,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Pubkey,
    pub dvp_id: [u8; 32],
    /// Bump of `dvp_authority` (`SwapDvp::dvp_authority`).
    pub bump: u8,
    pub create_proof: CreateDvpProof,
    pub spp: TransactIxData,
}

impl CreateDvp {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            creator,
            tree,
            dvp_id,
            bump,
            create_proof,
            spp,
        } = self;

        let dvp_authority = dvp_authority(&creator, &dvp_id, bump).map_err(err)?;
        // The create transact's only input is the address slot, whose
        // nullifier is the address: SPP writes its nullifier PDA.
        let nullifier_pdas =
            nullifier_pda_accounts(&tree, spp.inputs.iter().map(|input| &input.nullifier_hash));
        let input_trees =
            std::iter::repeat_n(AccountMeta::new(tree, false), spp.tree_contexts.len());
        let serialized_ix = wincode::serialize(&CreateDvpIxData {
            dvp_id,
            bump,
            create_proof,
            spp,
        })
        .map_err(err)?;

        // The payer and creator come first; everything after them is the
        // forwarded SPP transact list, ending in the `dvp_authority` owner
        // signer the program signs for (D8).
        let mut accounts = vec![
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(payer, true),
            AccountMeta::new(tree, false),
            AccountMeta::new_readonly(Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
            AccountMeta::new_readonly(Pubkey::default(), false),
        ];
        accounts.extend(input_trees);
        accounts.extend(nullifier_pdas);
        accounts.push(AccountMeta::new_readonly(dvp_authority, false));
        let mut instruction_data = vec![tag::CREATE_DVP];
        instruction_data.extend_from_slice(&serialized_ix);
        Ok(Instruction {
            program_id: dvp_program::ID,
            accounts,
            data: instruction_data,
        })
    }
}

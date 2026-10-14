use anyhow::Result;
use dvp_program::instructions::shared::dvp_authority;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use zolana_interface::{
    instruction::instruction_data::transact::TransactIxData, SHIELDED_POOL_PROGRAM_ID,
};
use zolana_program::instruction::nullifier_pda_accounts;

use crate::{err, tag, AbortDvpProof, CancelDvpIxData};

pub struct CancelDvp {
    pub payer: Pubkey,
    /// The terms' settlement authority, a readonly signer whose identity the
    /// abort proof binds in cancel mode.
    pub settlement_authority: Pubkey,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Pubkey,
    /// The terms' creator; with `dvp_id` and `bump` it seeds `dvp_authority`.
    pub creator: Pubkey,
    pub dvp_id: [u8; 32],
    /// Bump of `dvp_authority` (`SwapDvp::dvp_authority`).
    pub bump: u8,
    pub cancel_proof: AbortDvpProof,
    pub spp: TransactIxData,
}

impl CancelDvp {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            settlement_authority,
            tree,
            creator,
            dvp_id,
            bump,
            cancel_proof,
            spp,
        } = self;

        let dvp_authority = dvp_authority(&creator, &dvp_id, bump).map_err(err)?;
        // SPP writes a nullifier PDA for each input slot: the DvP UTXO, the
        // deposits and the random padding, which publishes a nullifier too.
        let nullifier_pdas =
            nullifier_pda_accounts(&tree, spp.inputs.iter().map(|input| &input.nullifier_hash));
        let input_trees =
            std::iter::repeat_n(AccountMeta::new(tree, false), spp.tree_contexts.len());
        let serialized_ix = wincode::serialize(&CancelDvpIxData {
            creator,
            dvp_id,
            bump,
            cancel_proof,
            spp,
        })
        .map_err(err)?;

        // The payer and settlement authority come first; everything after
        // them is the forwarded SPP transact list, ending in the
        // `dvp_authority` owner signer the program signs for (D8).
        let mut accounts = vec![
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(settlement_authority, true),
            AccountMeta::new(payer, true),
            AccountMeta::new(tree, false),
            AccountMeta::new_readonly(Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
            AccountMeta::new_readonly(Pubkey::default(), false),
        ];
        accounts.extend(input_trees);
        accounts.extend(nullifier_pdas);
        accounts.push(AccountMeta::new_readonly(dvp_authority, false));
        let mut instruction_data = vec![tag::CANCEL_DVP];
        instruction_data.extend_from_slice(&serialized_ix);
        Ok(Instruction {
            program_id: dvp_program::ID,
            accounts,
            data: instruction_data,
        })
    }
}

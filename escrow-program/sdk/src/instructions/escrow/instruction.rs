use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use zolana_interface::{
    instruction::{instruction_data::transact::TransactIxData, transact_nullifier_pda_accounts},
    SHIELDED_POOL_PROGRAM_ID,
};

use crate::{err, escrow_authority_pda, tag, EscrowIxData, EscrowProof};

pub struct Escrow {
    pub payer: Pubkey,
    /// Tree used to fetch the SPP inclusion and nullifier witnesses.
    pub input_tree: Pubkey,
    /// Tree that receives the SPP output UTXOs.
    pub output_tree: Pubkey,
    pub escrow_proof: EscrowProof,
    pub spp_proof: TransactIxData,
}

impl Escrow {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            input_tree,
            output_tree,
            escrow_proof,
            spp_proof,
        } = self;

        let input_trees = [input_tree];
        let nullifier_pdas = transact_nullifier_pda_accounts(&input_trees, spp_proof.inputs.iter());
        let serialized_ix = wincode::serialize(&EscrowIxData {
            proof: escrow_proof,
            transact: spp_proof,
        })
        .map_err(err)?;

        let mut accounts = vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(payer, true),
            AccountMeta::new(output_tree, false),
            AccountMeta::new_readonly(Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
            AccountMeta::new_readonly(Pubkey::default(), false),
            AccountMeta::new(input_tree, false),
        ];
        accounts.extend(nullifier_pdas);
        accounts.push(AccountMeta::new_readonly(escrow_authority_pda(), false));
        let mut instruction_data = vec![tag::ESCROW];
        instruction_data.extend_from_slice(&serialized_ix);
        Ok(Instruction {
            program_id: timelock_escrow_program::ID,
            accounts,
            data: instruction_data,
        })
    }
}

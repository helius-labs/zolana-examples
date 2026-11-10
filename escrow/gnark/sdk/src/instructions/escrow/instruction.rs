use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spr_interface::{
    instruction::instruction_data::transact::TransactIxData, SHIELDED_POOL_PROGRAM_ID,
};
use spr_program::instruction::nullifier_pda_accounts;

use crate::{err, escrow_authority_pda, tag, EscrowIxData, EscrowProof};

pub struct Escrow {
    /// The creator's ed25519 pubkey, a readonly signer: the timelock escrow
    /// program derives the creator's escrow-authority PDA from it, which owns
    /// the escrow UTXO, and binds the escrow terms to its owner identity.
    pub creator: Pubkey,
    pub payer: Pubkey,
    pub tree: Pubkey,
    pub escrow_proof: EscrowProof,
    pub spp_proof: TransactIxData,
}

impl Escrow {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            creator,
            payer,
            tree,
            escrow_proof,
            spp_proof,
        } = self;

        let nullifier_pdas = nullifier_pda_accounts(
            &tree,
            spp_proof.inputs.iter().map(|input| &input.nullifier_hash),
        );
        let serialized_ix = wincode::serialize(&EscrowIxData {
            proof: escrow_proof,
            transact: spp_proof,
        })
        .map_err(err)?;

        let mut accounts = vec![
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(payer, true),
            AccountMeta::new(tree, false),
            AccountMeta::new_readonly(Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID), false),
            AccountMeta::new_readonly(Pubkey::default(), false),
            AccountMeta::new(tree, false),
        ];
        accounts.extend(nullifier_pdas);
        accounts.push(AccountMeta::new_readonly(
            escrow_authority_pda(&creator),
            false,
        ));
        let mut instruction_data = vec![tag::ESCROW];
        instruction_data.extend_from_slice(&serialized_ix);
        Ok(Instruction {
            program_id: timelock_escrow_program::ID,
            accounts,
            data: instruction_data,
        })
    }
}

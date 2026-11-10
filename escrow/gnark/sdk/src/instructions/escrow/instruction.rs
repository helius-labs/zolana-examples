use anyhow::Result;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spr_interface::instruction::instruction_data::transact::TransactIxData;
use spr_program::instruction::Transact;

use crate::{err, escrow_authority_pda_and_bump, tag, EscrowIxData, EscrowProof};

pub struct Escrow {
    /// The creator's ed25519 pubkey. It owns the spent source, so it signs
    /// the SPP transact as its owner signer; the timelock escrow program
    /// derives the creator's escrow-authority PDA from it, which owns the
    /// escrow UTXO, and binds the escrow terms and the source to its owner
    /// identity.
    pub creator: Pubkey,
    pub payer: Pubkey,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Pubkey,
    pub escrow_proof: EscrowProof,
    pub spp_proof: TransactIxData,
}

impl Escrow {
    /// Accounts: the creator signer the program reads, then the SPP
    /// `transact` account list with the creator as owner signer of the
    /// source, then the creator's escrow-authority PDA, which the program
    /// signs for inside the CPI because SPP requires the owner of the
    /// data-bearing escrow output to sign.
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            creator,
            payer,
            tree,
            escrow_proof,
            spp_proof,
        } = self;

        let transact = Transact {
            payer,
            input_trees: vec![tree; spp_proof.tree_contexts.len()],
            output_tree: tree,
            owner_signers: vec![creator],
            interface_transfer_accounts: Vec::new(),
            data: spp_proof,
        };
        let (escrow_authority, authority_bump) = escrow_authority_pda_and_bump(&creator);
        let mut accounts = vec![AccountMeta::new_readonly(creator, true)];
        accounts.extend(transact.cpi_accounts(&[escrow_authority]));

        let serialized_ix = wincode::serialize(&EscrowIxData {
            proof: escrow_proof,
            authority_bump,
            transact: transact.data,
        })
        .map_err(err)?;
        let mut instruction_data = vec![tag::ESCROW];
        instruction_data.extend_from_slice(&serialized_ix);
        Ok(Instruction {
            program_id: timelock_escrow_program::ID,
            accounts,
            data: instruction_data,
        })
    }
}

use anyhow::Result;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use spr_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{escrow_authority_bump, shared::escrow_instruction, tag, EscrowIxData, EscrowProof};

pub struct Escrow {
    /// The creator's Solana pubkey. It owns the spent source, so it signs the
    /// SPP transact as its owner signer; the timelock escrow program derives
    /// the creator's escrow-authority PDA from it, which owns the escrow UTXO,
    /// and checks the escrow proof against its owner identity.
    pub creator: Address,
    /// Signs and pays the transaction and SPP. It may be the creator.
    pub payer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    pub escrow_proof: EscrowProof,
    /// The escrow UTXO address's viewing pubkey; see
    /// [`EscrowIxData::viewing_pubkey`].
    pub viewing_pubkey: [u8; 33],
    pub spp_proof: TransactIxData,
}

impl Escrow {
    /// Accounts: the creator signer the program reads, then the SPP
    /// `transact` account list with `payer` as the SPP payer and the creator
    /// as owner signer of the source, then the creator's escrow-authority
    /// PDA, which the program signs for inside the CPI because SPP requires
    /// the owner of the data-bearing escrow output to sign.
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            creator,
            payer,
            tree,
            escrow_proof,
            viewing_pubkey,
            spp_proof,
        } = self;
        let ix_data = EscrowIxData {
            proof: escrow_proof,
            viewing_pubkey,
            authority_bump: escrow_authority_bump(&creator),
            transact: spp_proof,
        };
        escrow_instruction(
            tag::ESCROW,
            vec![AccountMeta::new_readonly(creator, true)],
            vec![creator],
            creator,
            payer,
            tree,
            &ix_data.transact,
            &ix_data,
        )
    }
}

use anyhow::Result;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use spr_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{shared::escrow_instruction, tag, EscrowIxData, EscrowProof};

pub struct Escrow {
    /// The creator, which owns the spent source, signs and pays the
    /// transaction and SPP.
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
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            tree,
            escrow_proof,
            viewing_pubkey,
            spp_proof,
        } = self;
        let ix_data = EscrowIxData {
            proof: escrow_proof,
            viewing_pubkey,
            transact: spp_proof,
        };
        // The creator signs for the source it owns; its escrow-authority PDA
        // follows for the escrow UTXO's data.
        escrow_instruction(
            tag::ESCROW,
            vec![AccountMeta::new(payer, true)],
            vec![payer],
            payer,
            payer,
            tree,
            &ix_data.transact,
            &ix_data,
        )
    }
}

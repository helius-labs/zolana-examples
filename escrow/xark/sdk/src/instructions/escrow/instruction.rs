use anyhow::Result;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use spr_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{shared::escrow_instruction, tag, EscrowIxData, EscrowProof};

pub struct Escrow {
    /// The creator, which signs and pays the transaction and SPP.
    pub payer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    pub escrow_proof: EscrowProof,
    pub spp_proof: TransactIxData,
}

impl Escrow {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            tree,
            escrow_proof,
            spp_proof,
        } = self;
        let ix_data = EscrowIxData {
            proof: escrow_proof,
            transact: spp_proof,
        };
        escrow_instruction(
            tag::ESCROW,
            vec![AccountMeta::new(payer, true)],
            payer,
            tree,
            &ix_data.transact,
            &ix_data,
        )
    }
}

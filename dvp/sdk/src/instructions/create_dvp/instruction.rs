use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    instructions::{dvp_instruction, DvpAccounts},
    tag, CreateDvpIxData, CreateDvpProof,
};

pub struct CreateDvp {
    pub payer: Address,
    /// Creator checked by the proof and used in the PDA seeds.
    pub creator: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    pub dvp_id: [u8; 32],
    /// Bump of `dvp_authority` (`SwapDvp::dvp_authority`).
    pub bump: u8,
    pub viewing_pubkey: [u8; 33],
    pub create_proof: CreateDvpProof,
    pub spp: TransactIxData,
}

impl CreateDvp {
    /// 1. Encode the instruction data.
    /// 2. Add the DvP and SPP accounts.
    pub fn instruction(self) -> Result<Instruction> {
        // 1. Encode the instruction data.
        let Self {
            payer,
            creator,
            tree,
            dvp_id,
            bump,
            viewing_pubkey,
            create_proof,
            spp,
        } = self;
        let ix_data = CreateDvpIxData {
            dvp_id,
            bump,
            viewing_pubkey,
            create_proof,
            spp,
        };
        // 2. Add the DvP and SPP accounts.
        dvp_instruction(
            tag::CREATE_DVP,
            DvpAccounts {
                payer,
                signer: creator,
                tree,
                creator,
                dvp_id,
                bump,
            },
            &ix_data.spp,
            &ix_data,
        )
    }
}

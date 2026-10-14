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
    /// The terms' creator, a readonly signer whose identity the create proof
    /// binds. It also seeds `dvp_authority`.
    pub creator: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
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
        let ix_data = CreateDvpIxData {
            dvp_id,
            bump,
            create_proof,
            spp,
        };
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

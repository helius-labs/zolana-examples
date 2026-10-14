use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    instructions::{dvp_instruction, DvpAccounts},
    tag, AbortDvpProof, CancelDvpIxData,
};

pub struct CancelDvp {
    pub payer: Address,
    /// The terms' settlement authority, a readonly signer whose identity the
    /// abort proof binds in cancel mode.
    pub settlement_authority: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    /// The terms' creator; with `dvp_id` and `bump` it seeds `dvp_authority`.
    pub creator: Address,
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
        let ix_data = CancelDvpIxData {
            creator,
            dvp_id,
            bump,
            cancel_proof,
            spp,
        };
        dvp_instruction(
            tag::CANCEL_DVP,
            DvpAccounts {
                payer,
                signer: settlement_authority,
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

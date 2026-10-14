use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    instructions::{dvp_instruction, DvpAccounts},
    tag, ReclaimDvpIxData, ReclaimDvpProof,
};

pub struct ReclaimDvp {
    pub payer: Address,
    /// The reclaiming party, a readonly signer whose identity the reclaim
    /// proof binds: the seller for a delivery deposit, the buyer for a
    /// payment deposit.
    pub signer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    /// The terms' creator; with `dvp_id` and `bump` it seeds `dvp_authority`.
    pub creator: Address,
    pub dvp_id: [u8; 32],
    /// Bump of `dvp_authority` (`SwapDvp::dvp_authority`).
    pub bump: u8,
    pub reclaim_proof: ReclaimDvpProof,
    pub spp: TransactIxData,
}

impl ReclaimDvp {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            payer,
            signer,
            tree,
            creator,
            dvp_id,
            bump,
            reclaim_proof,
            spp,
        } = self;
        let ix_data = ReclaimDvpIxData {
            creator,
            dvp_id,
            bump,
            reclaim_proof,
            spp,
        };
        dvp_instruction(
            tag::RECLAIM_DVP,
            DvpAccounts {
                payer,
                signer,
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

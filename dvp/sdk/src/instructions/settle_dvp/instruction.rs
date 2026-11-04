use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    instructions::{dvp_instruction, DvpAccounts},
    tag, SettleDvpIxData, SettleDvpProof,
};

pub struct SettleDvp {
    pub payer: Address,
    /// Settlement authority checked by the proof.
    pub settlement_authority: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    /// The terms' creator; with `dvp_id` and `bump` it seeds `dvp_authority`.
    pub creator: Address,
    pub dvp_id: [u8; 32],
    /// Bump of `dvp_authority` (`SwapDvp::dvp_authority`).
    pub bump: u8,
    /// The time the settle proof checks the terms' window against; it must
    /// be within `TIME_WINDOW` behind the cluster clock when the transaction
    /// lands.
    pub current_timestamp: i64,
    pub settle_proof: SettleDvpProof,
    pub spp: TransactIxData,
}

impl SettleDvp {
    /// 1. Encode the instruction data.
    /// 2. Add the DvP and SPP accounts.
    pub fn instruction(self) -> Result<Instruction> {
        // 1. Encode the instruction data.
        let Self {
            payer,
            settlement_authority,
            tree,
            creator,
            dvp_id,
            bump,
            current_timestamp,
            settle_proof,
            spp,
        } = self;
        let ix_data = SettleDvpIxData {
            creator,
            dvp_id,
            bump,
            current_timestamp,
            settle_proof,
            spp,
        };
        // 2. Add the DvP and SPP accounts.
        dvp_instruction(
            tag::SETTLE_DVP,
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

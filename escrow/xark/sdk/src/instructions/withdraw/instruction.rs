use anyhow::Result;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use spr_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{shared::escrow_instruction, tag, WithdrawIxData, WithdrawProof};

pub struct Withdraw {
    /// The creator's Solana pubkey, a dedicated readonly signer the timelock
    /// escrow program checks against the withdraw proof's committed owner.
    pub creator: Address,
    /// Signs and pays the transaction and SPP.
    pub payer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
    pub withdraw_proof: WithdrawProof,
    pub unlock_timestamp: u64,
    pub spp_proof: TransactIxData,
}

impl Withdraw {
    pub fn instruction(self) -> Result<Instruction> {
        let Self {
            creator,
            payer,
            tree,
            withdraw_proof,
            unlock_timestamp,
            spp_proof,
        } = self;
        let ix_data = WithdrawIxData {
            proof: withdraw_proof,
            unlock_timestamp,
            transact: spp_proof,
        };
        // The creator is a dedicated readonly signer after the fee payer.
        escrow_instruction(
            tag::WITHDRAW,
            vec![
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(creator, true),
            ],
            payer,
            tree,
            &ix_data.transact,
            &ix_data,
        )
    }
}

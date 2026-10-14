use anyhow::Result;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_keypair::{P256Pubkey, ShieldedKeypair};
use zolana_program::instruction::Transact;
use zolana_transaction::{
    instructions::transact::{ConfidentialTransaction, SppProofInputs},
    WalletUtxo, SOL_MINT,
};

use crate::{
    err,
    state::{Deposit, Leg, SwapDvp},
};

/// Funding one leg: a plain SPP transfer of `amount` of the leg's mint from
/// the funder's private balance to [`Deposit::address`]. It goes through no
/// DvP instruction; the transfer's change returns to the funder. Anyone may
/// fund either leg, and may send more than the leg amount: settlement
/// refunds the surplus.
pub struct FundDvpParams {
    pub terms: SwapDvp,
    pub leg: Leg,
    pub amount: u64,
    /// The settlement authority's viewing pubkey, learned out of band (the
    /// terms name only its Solana address). The deposit ciphertext is
    /// encrypted to it.
    pub authority_viewing_pubkey: P256Pubkey,
    /// The funder's spendable notes, the first one real.
    pub inputs: Vec<WalletUtxo>,
    /// Fee payer and SPP payer; the funder's Solana key, since an Ed25519
    /// input owner must sign the transaction.
    pub payer: Address,
    pub tree_id: u16,
}

impl FundDvpParams {
    /// The encrypted transfer, ready to prove. The transaction viewing key
    /// derives from `funder`'s viewing key and the first input's nullifier,
    /// which is how the funder re-reads its deposit later
    /// ([`crate::index::scan_own_deposits`]).
    pub fn build(self, funder: &ShieldedKeypair) -> Result<SppProofInputs> {
        let recipient = Deposit::address(&self.terms, self.authority_viewing_pubkey)?;
        let mut transaction = ConfidentialTransaction::new(self.inputs, self.payer)
            .map_err(err)?
            .with_output_tree_id(self.tree_id)
            .map_err(err)?;
        // `transfer` rejects SOL_MINT, so a SOL leg goes through
        // `transfer_sol`.
        let mint = self.leg.mint(&self.terms);
        if mint == SOL_MINT {
            transaction.transfer_sol(&recipient, self.amount)
        } else {
            transaction.transfer(&recipient, mint, self.amount)
        }
        .map_err(err)?;
        transaction.encrypt(funder).map_err(err)
    }
}

/// The SPP `transact` instruction of a proven funding transfer.
pub struct FundDvp {
    pub payer: Pubkey,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Pubkey,
    pub spp: TransactIxData,
}

impl FundDvp {
    pub fn instruction(self) -> Instruction {
        Transact {
            payer: self.payer,
            input_trees: vec![self.tree; self.spp.tree_contexts.len()],
            output_tree: self.tree,
            owner_signers: Vec::new(),
            interface_transfer_accounts: Vec::new(),
            data: self.spp,
        }
        .instruction()
    }
}

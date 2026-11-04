use anyhow::{ensure, Result};
use solana_address::Address;
use solana_instruction::Instruction;
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
/// DvP instruction; the transfer's change returns to the funder. The funder
/// may send more than the leg amount: settlement refunds the surplus to the
/// leg's refund destination. Only the leg's party (the seller for delivery,
/// the buyer for payment) can reclaim a deposit, so anyone else who funds a
/// leg gives up control of it.
pub struct FundDvpParams {
    pub terms: SwapDvp,
    pub leg: Leg,
    pub amount: u64,
    /// The settlement authority's viewing pubkey, learned out of band (the
    /// terms name only its Solana address). The deposit ciphertext is
    /// encrypted to it.
    pub authority_viewing_pubkey: P256Pubkey,
    /// The funder's spendable UTXOs, the first one real.
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
    /// ([`crate::index::scan_own_deposits`]). An amount below the leg's could
    /// never settle (the deposit would have to be reclaimed and the leg
    /// funded again), so it is refused.
    pub fn build(self, funder: &ShieldedKeypair) -> Result<SppProofInputs> {
        let leg_amount = self.leg.amount(&self.terms);
        ensure!(
            self.amount >= leg_amount,
            "funding {:?} with {} below its amount {leg_amount}",
            self.leg,
            self.amount
        );
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
    pub payer: Address,
    /// The SPP tree: output tree, and input tree of every tree context.
    pub tree: Address,
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

#[cfg(test)]
mod tests {
    use zolana_transaction::Mint;

    use super::*;
    use crate::index::fixture::{
        fixture_parties, pubkey, sample_terms, wallet_utxo, FIXTURE_TREE_ID,
    };

    #[test]
    fn build_refuses_less_than_the_leg_amount() {
        let parties = fixture_parties();
        let terms = sample_terms(&parties);
        let [_, buyer, authority] = &parties;
        let leg_amount = Leg::Payment.amount(&terms);
        let params = |amount| FundDvpParams {
            terms: terms.clone(),
            leg: Leg::Payment,
            amount,
            authority_viewing_pubkey: authority.viewing_pubkey(),
            inputs: vec![wallet_utxo(buyer, Mint::SOL, 10 * leg_amount)],
            payer: pubkey(buyer),
            tree_id: FIXTURE_TREE_ID,
        };
        let error = params(leg_amount - 1)
            .build(buyer)
            .err()
            .expect("an underfunded leg");
        assert_eq!(
            error.to_string(),
            format!(
                "funding Payment with {} below its amount {leg_amount}",
                leg_amount - 1
            )
        );
        assert!(params(leg_amount).build(buyer).is_ok());
    }
}

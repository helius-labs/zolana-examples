use anyhow::Result;
use dvp_program::circuits::{
    ReclaimDvp as ReclaimDvpInputs, ReclaimDvpPrivateInputs, ReclaimDvpPublicInputs,
};
use solana_address::Address;
use zolana_client::ZolanaIndexer;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::{ShieldedAddress, ShieldedKeypair};
use zolana_program::{ProgramOwner, TxContext, ZkProgramCircuit};
use zolana_transaction::instructions::transact::SppProofInputs;

use super::ReclaimDvp;
use crate::{
    err,
    shared::{ensure_output_tree, prove_pda_transact},
    state::{Deposit, Leg},
};

pub struct ReclaimDvpTransactionParams {
    pub deposit: Deposit,
    pub payer: Address,
    pub refund_destination: Option<ShieldedAddress>,
    pub tree_id: u16,
}

pub struct ReclaimDvpTransaction {
    signer: Address,
    creator: Address,
    bump: u8,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ReclaimDvpInputs,
}

impl ReclaimDvpTransactionParams {
    /// 1. Check the deposit and refund destination.
    /// 2. Build the program transaction.
    /// 3. Encrypt the refund UTXO.
    pub fn build(self, signer: &ShieldedKeypair) -> Result<ReclaimDvpTransaction> {
        // 1. Check the deposit and refund destination.
        let terms = &self.deposit.terms;
        self.deposit.check_leg(terms, self.deposit.leg)?;
        ensure_output_tree("the deposit", self.deposit.tree_id, self.tree_id)?;
        let sender = signer.shielded_address().map_err(err)?;
        let refund_destination = self.refund_destination.unwrap_or(sender);
        refund_destination.signing_pubkey.curve().map_err(err)?;
        let signer_address = sender.solana_address().map_err(err)?;
        let creator = terms.creator;
        let (pda, bump) = terms.dvp_authority()?;
        let mut circuit_terms = dvp_program::circuits::SwapDvpTerms::try_from(terms)?;
        match self.deposit.leg {
            Leg::Delivery => circuit_terms.delivery_mint = self.deposit.mint,
            Leg::Payment => circuit_terms.payment_mint = self.deposit.mint,
        }
        // 2. Build the program transaction.
        let proof_inputs = ReclaimDvpInputs {
            private: ReclaimDvpPrivateInputs {
                tx_context: TxContext::new().with_output_tree_id(Some(self.tree_id)),
                terms: circuit_terms,
                deposit: self.deposit.to_input_utxo()?,
                payment_leg: self.deposit.leg == Leg::Payment,
                refund_destination,
            },
            public: ReclaimDvpPublicInputs {
                creator_identity: solana_owner_identity(creator.as_array()).map_err(err)?,
                dvp_id: terms.dvp_id()?,
                dvp_owner: ProgramOwner::new(pda)
                    .address(signer.viewing_pubkey())
                    .map_err(err)?,
                signer_identity: solana_owner_identity(signer_address.as_array()).map_err(err)?,
            },
        };
        let finalized = proof_inputs
            .create_finalized_transaction(&sender, self.payer)
            .map_err(err)?;
        // 3. Encrypt the refund UTXO.
        let key = signer
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(ReclaimDvpTransaction {
            signer: signer_address,
            creator,
            bump,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl ReclaimDvpTransaction {
    /// 1. Prove the SPP transaction.
    /// 2. Prove reclaim_dvp and build the instruction.
    pub fn prove(self, indexer: &ZolanaIndexer, tree: Address) -> Result<ReclaimDvp> {
        let private_tx_hash = self
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .map_err(err)?;
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp = prove_pda_transact(indexer, self.spp_proof_inputs, &private_tx_hash)?;
        // 2. Prove reclaim_dvp and build the instruction.
        let reclaim_proof =
            crate::proof::prove(crate::proof::CircuitId::ReclaimDvp, &self.proof_inputs)?;
        Ok(ReclaimDvp {
            payer,
            signer: self.signer,
            tree,
            creator: self.creator,
            dvp_id: self.proof_inputs.public.dvp_id,
            bump: self.bump,
            reclaim_proof,
            spp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::fixture::{deposit_fixture, shielded_transaction, spendable_amounts};

    #[test]
    fn chosen_destination_decrypts_the_full_reclaimed_deposit() {
        // 1. Reclaim payment to a chosen destination.
        let fixture = deposit_fixture();
        let [_, buyer] = &fixture.funders;
        let [_, payment] = &fixture.deposits;
        let transaction = ReclaimDvpTransactionParams {
            deposit: payment.clone(),
            payer: fixture.terms.creator,
            refund_destination: Some(fixture.authority.shielded_address().unwrap()),
            tree_id: payment.tree_id,
        }
        .build(buyer)
        .unwrap();
        let tx = shielded_transaction(&transaction.spp_proof_inputs, 30);
        // 2. Check the recipient decrypts the entire refund and the buyer cannot.
        assert_eq!(
            (
                spendable_amounts(
                    &fixture.authority,
                    &tx,
                    &fixture.registry,
                    &payment.mint.asset
                ),
                spendable_amounts(buyer, &tx, &fixture.registry, &payment.mint.asset)
            ),
            (vec![payment.amount], vec![])
        );
    }

    #[test]
    fn builder_rejects_the_other_party() {
        // 1. Check the buyer cannot reclaim the seller's delivery deposit.
        let fixture = deposit_fixture();
        let [_, buyer] = &fixture.funders;
        let [delivery, _] = &fixture.deposits;
        let params = || ReclaimDvpTransactionParams {
            deposit: delivery.clone(),
            payer: fixture.terms.creator,
            refund_destination: None,
            tree_id: delivery.tree_id,
        };
        assert!(params().build(buyer).is_err());
    }
}

use anyhow::{ensure, Result};
use dvp_program::circuits::{
    AbortDvp as AbortDvpInputs, AbortDvpPrivateInputs, AbortDvpPublicInputs,
};
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_client::ZolanaIndexer;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::ShieldedKeypair;
use zolana_program::{ProgramOwner, TxContext, ZkProgramCircuit};
use zolana_transaction::{instructions::transact::SppProofInputs, Mint, WalletUtxo};

use super::AbortMode;
use crate::{
    err,
    instructions::{cancel_dvp::CancelDvp, reject_dvp::RejectDvp},
    shared::{ensure_output_tree, prove_pda_transact},
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};

pub struct AbortDvpTransactionParams {
    pub terms: SwapDvp,
    pub mode: AbortMode,
    pub dvp_utxo: DvpUtxo,
    pub dvp_utxo_leaf_index: u64,
    pub delivery_mint: Mint,
    pub payment_mint: Mint,
    pub delivery_deposit: Option<Deposit>,
    pub payment_deposit: Option<Deposit>,
    pub payer: Address,
    pub tree_id: u16,
}

pub struct AbortDvpTransaction {
    mode: AbortMode,
    signer: Address,
    creator: Address,
    bump: u8,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: AbortDvpInputs,
}

pub enum AbortDvp {
    Cancel(CancelDvp),
    Reject(RejectDvp),
}
impl AbortDvp {
    pub fn instruction(self) -> Result<Instruction> {
        match self {
            Self::Cancel(cancel) => cancel.instruction(),
            Self::Reject(reject) => reject.instruction(),
        }
    }
}

impl AbortDvpTransactionParams {
    /// 1. Check the terms, deposits, and input trees.
    /// 2. Build the program transaction with optional deposits.
    /// 3. Encrypt both refund UTXOs, including zero amounts.
    pub fn build(self, signer: &ShieldedKeypair) -> Result<AbortDvpTransaction> {
        // 1. Check the terms, deposits, and input trees.
        self.terms.validate()?;
        ensure!(self.dvp_utxo.terms == self.terms, "DvP UTXO of other terms");
        ensure_output_tree("the DvP UTXO", self.dvp_utxo.tree_id, self.tree_id)?;
        for (deposit, leg, mint) in [
            (&self.delivery_deposit, Leg::Delivery, self.delivery_mint),
            (&self.payment_deposit, Leg::Payment, self.payment_mint),
        ] {
            if let Some(deposit) = deposit {
                deposit.check_leg(&self.terms, leg)?;
                ensure!(
                    deposit.mint == mint,
                    "deposit mint differs from the refund mint"
                );
                ensure_output_tree("a deposit", deposit.tree_id, self.tree_id)?;
            }
        }
        let sender = signer.shielded_address().map_err(err)?;
        let signer_address = sender.solana_address().map_err(err)?;
        let creator = self.terms.creator;
        let (pda, bump) = self.terms.dvp_authority()?;
        let deposit_input = |deposit: Option<Deposit>| -> Result<WalletUtxo> {
            match deposit {
                Some(deposit) => deposit.to_input_utxo(),
                None => WalletUtxo::dummy(self.tree_id).map_err(err),
            }
        };
        // 2. Build the program transaction with optional deposits.
        let proof_inputs = AbortDvpInputs {
            private: AbortDvpPrivateInputs {
                tx_context: TxContext::new()
                    .with_blinding_seed(self.terms.abort_blinding_seed()?)
                    .with_output_tree_id(Some(self.tree_id)),
                terms: self
                    .terms
                    .circuit_terms(self.delivery_mint, self.payment_mint)?,
                dvp_utxo: self.dvp_utxo.to_input_utxo(self.dvp_utxo_leaf_index)?,
                delivery_deposit: deposit_input(self.delivery_deposit)?,
                payment_deposit: deposit_input(self.payment_deposit)?,
                buyer_rejects: signer_address == self.terms.buyer,
            },
            public: AbortDvpPublicInputs {
                creator_identity: solana_owner_identity(creator.as_array()).map_err(err)?,
                dvp_id: self.terms.dvp_id()?,
                dvp_owner: ProgramOwner::new(pda)
                    .address(signer.viewing_pubkey())
                    .map_err(err)?,
                signer_identity: solana_owner_identity(signer_address.as_array()).map_err(err)?,
                reject: self.mode == AbortMode::Reject,
            },
        };
        let finalized = proof_inputs
            .create_finalized_transaction(&sender, self.payer)
            .map_err(err)?;
        // 3. Encrypt both refund UTXOs, including zero amounts.
        let key = signer
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(AbortDvpTransaction {
            mode: self.mode,
            signer: signer_address,
            creator,
            bump,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl AbortDvpTransaction {
    /// 1. Prove the SPP transaction.
    /// 2. Prove abort_dvp and build cancel_dvp or reject_dvp.
    pub fn prove(self, indexer: &ZolanaIndexer, tree: Address) -> Result<AbortDvp> {
        let private_tx_hash = self
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .map_err(err)?;
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp = prove_pda_transact(indexer, self.spp_proof_inputs, &private_tx_hash)?;
        // 2. Prove abort_dvp and build cancel_dvp or reject_dvp.
        let proof = crate::proof::prove(crate::proof::CircuitId::AbortDvp, &self.proof_inputs)?;
        let dvp_id = self.proof_inputs.public.dvp_id;
        Ok(match self.mode {
            AbortMode::Cancel => AbortDvp::Cancel(CancelDvp {
                payer,
                settlement_authority: self.signer,
                tree,
                creator: self.creator,
                dvp_id,
                bump: self.bump,
                cancel_proof: proof,
                spp,
            }),
            AbortMode::Reject => AbortDvp::Reject(RejectDvp {
                payer,
                signer: self.signer,
                tree,
                creator: self.creator,
                dvp_id,
                bump: self.bump,
                reject_proof: proof,
                spp,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::fixture::{
        deposit_fixture, shielded_transaction, spendable_amounts, DepositFixture,
        FIXTURE_LEAF_INDEX,
    };

    #[test]
    fn refunds_decrypt_in_spec_order_for_every_funding_combination() {
        let fixture = deposit_fixture();
        for (delivery, payment) in [(false, false), (true, false), (false, true), (true, true)] {
            // 1. Build an abort with optional deposits.
            let transaction = params(&fixture, delivery, payment)
                .build(&fixture.authority)
                .unwrap();
            let tx = shielded_transaction(&transaction.spp_proof_inputs, 40);
            let [seller, buyer] = &fixture.funders;
            // 2. Check real refunds remain decryptable even at zero amount.
            assert_eq!(
                (
                    spendable_amounts(seller, &tx, &fixture.registry, &fixture.terms.delivery_mint),
                    spendable_amounts(buyer, &tx, &fixture.registry, &fixture.terms.payment_mint)
                ),
                (
                    vec![if delivery {
                        fixture.deposits[0].amount
                    } else {
                        0
                    }],
                    vec![if payment {
                        fixture.deposits[1].amount
                    } else {
                        0
                    }]
                )
            );
        }
    }

    #[test]
    fn cancel_rejects_party_signers_and_reject_requires_a_party() {
        // 1. Check only the settlement authority cancels.
        let fixture = deposit_fixture();
        for party in &fixture.funders {
            assert!(params(&fixture, true, true).build(party).is_err());
        }
        // 2. Check the settlement authority cannot reject.
        assert!(AbortDvpTransactionParams {
            mode: AbortMode::Reject,
            ..params(&fixture, true, true)
        }
        .build(&fixture.authority)
        .is_err());
    }

    fn params(
        fixture: &DepositFixture,
        delivery: bool,
        payment: bool,
    ) -> AbortDvpTransactionParams {
        let [delivery_deposit, payment_deposit] = &fixture.deposits;
        AbortDvpTransactionParams {
            terms: fixture.terms.clone(),
            mode: AbortMode::Cancel,
            dvp_utxo: DvpUtxo::new(fixture.terms.clone(), delivery_deposit.tree_id),
            dvp_utxo_leaf_index: FIXTURE_LEAF_INDEX,
            delivery_mint: delivery_deposit.mint,
            payment_mint: payment_deposit.mint,
            delivery_deposit: delivery.then(|| delivery_deposit.clone()),
            payment_deposit: payment.then(|| payment_deposit.clone()),
            payer: fixture.terms.creator,
            tree_id: delivery_deposit.tree_id,
        }
    }
}

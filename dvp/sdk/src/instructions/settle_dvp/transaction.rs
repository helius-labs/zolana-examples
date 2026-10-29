use anyhow::{ensure, Result};
use dvp_program::circuits::{
    SettleDvp as SettleDvpInputs, SettleDvpPrivateInputs, SettleDvpPublicInputs,
};
use solana_address::Address;
use zolana_client::ZolanaIndexer;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::ShieldedKeypair;
use zolana_program::{ProgramOwner, TxContext, ZkProgram};
use zolana_transaction::instructions::transact::SppProofInputs;

use super::SettleDvp;
use crate::{
    err,
    shared::{ensure_output_tree, prove_pda_transact},
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};

pub struct SettleDvpTransactionParams {
    pub terms: SwapDvp,
    pub current_timestamp: i64,
    pub dvp_utxo: DvpUtxo,
    pub dvp_utxo_leaf_index: u64,
    pub delivery_deposit: Deposit,
    pub payment_deposit: Deposit,
    pub payer: Address,
    pub tree_id: u16,
}

pub struct SettleDvpTransaction {
    settlement_authority: Address,
    creator: Address,
    bump: u8,
    current_timestamp: i64,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: SettleDvpInputs,
}

impl SettleDvpTransactionParams {
    /// 1. Check the terms, deposits, and input trees.
    /// 2. Build the program transaction.
    /// 3. Encrypt the settlement and change UTXOs.
    pub fn build(self, authority: &ShieldedKeypair) -> Result<SettleDvpTransaction> {
        // 1. Check the terms, deposits, and input trees.
        self.terms.validate()?;
        ensure!(self.dvp_utxo.terms == self.terms, "DvP UTXO of other terms");
        self.delivery_deposit
            .check_leg(&self.terms, Leg::Delivery)?;
        self.payment_deposit.check_leg(&self.terms, Leg::Payment)?;
        ensure_output_tree("the DvP UTXO", self.dvp_utxo.tree_id, self.tree_id)?;
        ensure_output_tree(
            "the delivery deposit",
            self.delivery_deposit.tree_id,
            self.tree_id,
        )?;
        ensure_output_tree(
            "the payment deposit",
            self.payment_deposit.tree_id,
            self.tree_id,
        )?;
        let signer = authority.shielded_address().map_err(err)?;
        let settlement_authority = signer.solana_address().map_err(err)?;
        let creator = self.terms.creator;
        let (pda, bump) = self.terms.dvp_authority()?;
        // 2. Build the program transaction.
        let proof_inputs = SettleDvpInputs {
            private: SettleDvpPrivateInputs {
                tx_context: TxContext::new()
                    .with_blinding_seed(self.terms.settle_blinding_seed()?)
                    .with_output_tree_id(Some(self.tree_id)),
                terms: self
                    .terms
                    .circuit_terms(self.delivery_deposit.mint, self.payment_deposit.mint)?,
                dvp_utxo: self.dvp_utxo.to_input_utxo(self.dvp_utxo_leaf_index)?,
                delivery_deposit: self.delivery_deposit.to_input_utxo()?,
                payment_deposit: self.payment_deposit.to_input_utxo()?,
            },
            public: SettleDvpPublicInputs {
                creator_identity: solana_owner_identity(creator.as_array()).map_err(err)?,
                dvp_id: self.terms.dvp_id()?,
                dvp_owner: ProgramOwner::new(pda)
                    .address(authority.viewing_pubkey())
                    .map_err(err)?,
                settlement_authority_identity: solana_owner_identity(
                    settlement_authority.as_array(),
                )
                .map_err(err)?,
                current_timestamp: u64::try_from(self.current_timestamp)?,
            },
        };
        let finalized = proof_inputs
            .create_finalized_transaction(&signer, self.payer)
            .map_err(err)?;
        // 3. Encrypt the settlement and change UTXOs.
        let key = authority
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(SettleDvpTransaction {
            settlement_authority,
            creator,
            bump,
            current_timestamp: self.current_timestamp,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl SettleDvpTransaction {
    /// 1. Prove the SPP transaction.
    /// 2. Prove settle_dvp and build the instruction.
    pub fn prove(self, indexer: &ZolanaIndexer, tree: Address) -> Result<SettleDvp> {
        let private_tx_hash = self
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .map_err(err)?;
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp = prove_pda_transact(indexer, self.spp_proof_inputs, &private_tx_hash)?;
        // 2. Prove settle_dvp and build the instruction.
        let settle_proof =
            crate::proof::prove(crate::proof::CircuitId::SettleDvp, &self.proof_inputs)?;
        Ok(SettleDvp {
            payer,
            settlement_authority: self.settlement_authority,
            tree,
            creator: self.creator,
            dvp_id: self.proof_inputs.public.dvp_id,
            bump: self.bump,
            current_timestamp: self.current_timestamp,
            settle_proof,
            spp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::fixture::{
        deposit_fixture, shielded_transaction, spendable_amounts, DepositFixture,
        FIXTURE_LEAF_INDEX, FIXTURE_SURPLUS,
    };

    #[test]
    fn parties_decrypt_only_their_settlement_and_change() {
        // 1. Build and encrypt the settlement.
        let fixture = deposit_fixture();
        let transaction = params(&fixture).build(&fixture.authority).unwrap();
        let tx = shielded_transaction(&transaction.spp_proof_inputs, 40);
        let [seller, buyer] = &fixture.funders;
        // 2. Check each party receives its agreed asset and surplus.
        assert_eq!(
            (
                spendable_amounts(seller, &tx, &fixture.registry, &fixture.terms.delivery_mint),
                spendable_amounts(seller, &tx, &fixture.registry, &fixture.terms.payment_mint),
                spendable_amounts(buyer, &tx, &fixture.registry, &fixture.terms.delivery_mint),
                spendable_amounts(buyer, &tx, &fixture.registry, &fixture.terms.payment_mint),
            ),
            (
                vec![FIXTURE_SURPLUS],
                vec![fixture.terms.payment_amount],
                vec![fixture.terms.delivery_amount],
                vec![FIXTURE_SURPLUS]
            )
        );
    }

    #[test]
    fn builder_rejects_the_wrong_authority_and_input_tree() {
        // 1. Check authorization before proving.
        let fixture = deposit_fixture();
        assert!(params(&fixture).build(&fixture.funders[0]).is_err());
        // 2. Check the instruction's single tree contains every input.
        let mut params = params(&fixture);
        params.tree_id += 1;
        assert!(params.build(&fixture.authority).is_err());
    }

    fn params(fixture: &DepositFixture) -> SettleDvpTransactionParams {
        let [delivery, payment] = &fixture.deposits;
        SettleDvpTransactionParams {
            terms: fixture.terms.clone(),
            current_timestamp: 1_000,
            dvp_utxo: DvpUtxo::new(fixture.terms.clone(), delivery.tree_id),
            dvp_utxo_leaf_index: FIXTURE_LEAF_INDEX,
            delivery_deposit: delivery.clone(),
            payment_deposit: payment.clone(),
            payer: fixture.terms.creator,
            tree_id: delivery.tree_id,
        }
    }
}

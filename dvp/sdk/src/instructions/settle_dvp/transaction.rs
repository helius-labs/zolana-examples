use anyhow::{ensure, Result};
use dvp_prover::SettleDvpProofInputs;
use solana_address::Address;
use zolana_client::ZolanaIndexer;
use zolana_keypair::{NullifierKey, ShieldedKeypair};
use zolana_transaction::instructions::transact::SppProofInputs;

use super::{SettleDvp, SettleDvpDerivation, SettleDvpProofInputParams};
use crate::{
    encryption::encrypt_outputs,
    err,
    prover::DvpProverClient,
    state::{Deposit, DvpUtxo, SwapDvp},
};

/// Everything the settle transaction needs besides the settlement
/// authority's keys and the prover.
pub struct SettleDvpTransactionParams {
    pub terms: SwapDvp,
    /// The time the settle proof checks the terms' window against. The
    /// program accepts it while it trails the cluster clock by at most
    /// `TIME_WINDOW` seconds and is not ahead of it, so take a recent
    /// cluster time, a few seconds back.
    pub current_timestamp: i64,
    pub dvp_utxo: DvpUtxo,
    /// Leaf index of the DvP UTXO in its tree (`DiscoveredDvp::leaf_index`).
    pub dvp_utxo_leaf_index: u64,
    pub delivery_deposit: Deposit,
    pub payment_deposit: Deposit,
    /// Pays the transaction and is the SPP payer, the first signer the SPP
    /// proof binds.
    pub payer: Address,
    /// Raw id of the tree the four outputs are appended to.
    pub tree_id: u16,
}

/// The settle transaction before its proofs: the SPP proof inputs and the
/// settle proof inputs over the same transaction.
pub struct SettleDvpTransaction {
    settlement_authority: Address,
    creator: Address,
    bump: u8,
    current_timestamp: i64,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: SettleDvpProofInputs,
}

impl SettleDvpTransactionParams {
    /// Builds the settle `transact` at SPP shape 3x4 from
    /// [`SettleDvpProofInputParams::derive`]: the DvP UTXO and both deposits
    /// in, the two settlement legs and the two change outputs out, every
    /// blinding derived from the terms' settlement seed.
    ///
    /// Each output is encrypted to its destination, so the buyer, the seller
    /// and the refund destinations find their notes with their own viewing
    /// keys. The transaction viewing key is derived from the settlement
    /// authority's viewing key and the DvP UTXO's nullifier, the first
    /// nullifier.
    ///
    /// Errors unless `settlement_authority` is the terms' settlement
    /// authority, `current_timestamp` is in the settlement window and both
    /// deposits fund their legs in full.
    pub fn build(self, settlement_authority: &ShieldedKeypair) -> Result<SettleDvpTransaction> {
        let Self {
            terms,
            current_timestamp,
            dvp_utxo,
            dvp_utxo_leaf_index,
            delivery_deposit,
            payment_deposit,
            payer,
            tree_id,
        } = self;
        let authority = settlement_authority
            .shielded_address()
            .map_err(err)?
            .solana_address()
            .map_err(err)?;
        let creator = terms.creator;
        let (_, bump) = terms.dvp_authority()?;
        let SettleDvpDerivation {
            inputs,
            outputs,
            blinding_seed,
            proof_inputs,
        } = SettleDvpProofInputParams {
            terms,
            settlement_authority: authority,
            current_timestamp,
            dvp_utxo,
            dvp_utxo_leaf_index,
            delivery_deposit,
            payment_deposit,
            output_tree_id: tree_id,
        }
        .derive()?;

        let [dvp_input, ..] = &inputs;
        let tx_viewing_key = settlement_authority
            .get_transaction_viewing_key(&dvp_input.nullifier)
            .map_err(err)?;
        let spp_proof_inputs = SppProofInputs {
            external_data: encrypt_outputs(&outputs, &tx_viewing_key, tree_id)?,
            input_utxos: inputs.into(),
            output_utxos: outputs.into(),
            payer,
            blinding_seed,
            output_tree_id: tree_id,
            cache_accounts: Default::default(),
        };
        Ok(SettleDvpTransaction {
            settlement_authority: authority,
            creator,
            bump,
            current_timestamp,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl SettleDvpTransaction {
    /// Proves the SPP transaction through `indexer` (which serves the
    /// inclusion proofs of the DvP UTXO and the deposits) and the settle
    /// proof locally, and returns the instruction builder for `tree`, the
    /// account of the tree with id `tree_id`.
    pub fn prove(self, indexer: &ZolanaIndexer, tree: Address) -> Result<SettleDvp> {
        let Self {
            settlement_authority,
            creator,
            bump,
            current_timestamp,
            spp_proof_inputs,
            proof_inputs,
        } = self;
        let payer = spp_proof_inputs.payer;
        // Every input is owned by the PDA with the public zero secret.
        let spp = indexer
            .prove_transact(spp_proof_inputs, &NullifierKey::from_secret([0; 31]))
            .map_err(err)?;
        ensure!(
            spp.private_tx_hash == proof_inputs.private_tx_hash,
            "the SPP proof and the settle proof cover different transactions"
        );
        let settle_proof = DvpProverClient::new()
            .prove_settle_dvp(&proof_inputs)?
            .into();
        Ok(SettleDvp {
            payer,
            settlement_authority,
            tree,
            creator,
            dvp_id: proof_inputs.dvp_id,
            bump,
            current_timestamp,
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

    /// Leaf the fixture settle appends its first output at.
    const SETTLE_LEAF: u64 = 40;

    fn params(fixture: &DepositFixture) -> SettleDvpTransactionParams {
        let [delivery, payment] = &fixture.deposits;
        SettleDvpTransactionParams {
            terms: fixture.terms.clone(),
            current_timestamp: 1_000,
            dvp_utxo: DvpUtxo::new(fixture.terms.clone(), delivery.tree_id),
            dvp_utxo_leaf_index: FIXTURE_LEAF_INDEX,
            delivery_deposit: delivery.clone(),
            payment_deposit: payment.clone(),
            payer: Address::new_from_array([7; 32]),
            tree_id: delivery.tree_id,
        }
    }

    #[test]
    fn build_pays_both_legs_and_returns_the_surplus() {
        let fixture = deposit_fixture();
        let [seller, buyer] = &fixture.funders;
        let terms = &fixture.terms;
        let transaction = params(&fixture).build(&fixture.authority).expect("build");
        let inputs = &transaction.spp_proof_inputs;
        let tx = shielded_transaction(inputs, SETTLE_LEAF);
        let amounts = |owner: &ShieldedKeypair, mint: &Address| {
            spendable_amounts(owner, &tx, &fixture.registry, mint)
        };
        assert_eq!(
            (
                inputs.input_utxos.len(),
                inputs.external_data.outputs.len(),
                inputs.private_tx_blinding().expect("blinding"),
                amounts(buyer, &terms.delivery_mint),
                amounts(seller, &terms.payment_mint),
                amounts(seller, &terms.delivery_mint),
                amounts(buyer, &terms.payment_mint),
                amounts(&fixture.authority, &terms.delivery_mint),
            ),
            (
                3,
                4,
                transaction.proof_inputs.private_tx_blinding,
                vec![terms.delivery_amount],
                vec![terms.payment_amount],
                vec![FIXTURE_SURPLUS],
                vec![FIXTURE_SURPLUS],
                Vec::new(),
            )
        );
    }

    #[test]
    fn build_refuses_a_signer_other_than_the_settlement_authority() {
        let fixture = deposit_fixture();
        let [seller, buyer] = &fixture.funders;
        assert!(params(&fixture).build(seller).is_err());
        assert!(params(&fixture).build(buyer).is_err());
    }
}

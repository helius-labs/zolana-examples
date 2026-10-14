use anyhow::{anyhow, ensure, Result};
use dvp_program::instructions::{
    settle_dvp::SettleDvpPublicInput, shared::dvp_authority_owner_hash,
};
use dvp_prover::{ProofInputUtxo, SettleDvpProofInputs, SwapDvpProofInput};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_keypair::ShieldedAddress;
use zolana_program::{derive_private_tx_blinding, PrivateTxHash};
use zolana_transaction::{utxo::SppProofInputUtxo, Mint, SppProofOutputUtxo};

use crate::{
    err,
    shared::assign_output_blindings,
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};

/// Everything the settle proof needs. The 3x4 transaction spends the DvP
/// UTXO and both deposits and pays out both legs plus their surplus; every
/// blinding derives from the terms' settlement seed.
pub struct SettleDvpProofInputParams {
    pub terms: SwapDvp,
    /// The signer; must be the terms' settlement authority.
    pub settlement_authority: Address,
    /// The settle instruction's timestamp. It must lie in the settlement
    /// window of the terms and be non-negative (D5).
    pub current_timestamp: i64,
    pub dvp_utxo: DvpUtxo,
    /// Leaf index of the DvP UTXO in its tree (`DiscoveredDvp::leaf_index`).
    pub dvp_utxo_leaf_index: u64,
    /// Funds the delivery leg with at least its amount.
    pub delivery_deposit: Deposit,
    /// Funds the payment leg with at least its amount.
    pub payment_deposit: Deposit,
    /// Raw id of the tree the four outputs are appended to.
    pub output_tree_id: u16,
}

/// The settle transaction as both proofs see it: the SPP inputs and outputs
/// (owner addresses and derived blindings set, ready to encrypt) and the
/// settle witness over the same values.
pub struct SettleDvpDerivation {
    /// `[DvP UTXO, delivery deposit, payment deposit]`.
    pub inputs: [SppProofInputUtxo; 3],
    /// `[buyer settlement, seller settlement, seller refund, buyer refund]`.
    pub outputs: [SppProofOutputUtxo; 4],
    /// `Poseidon(SETTLEMENT_TAG, terms.blinding)`.
    pub blinding_seed: [u8; 32],
    pub proof_inputs: SettleDvpProofInputs,
}

impl SettleDvpProofInputParams {
    /// Checks the authority, the settlement window and both deposits, then
    /// derives the transaction:
    ///
    /// - inputs: the DvP UTXO and the two deposits, all owned by the DvP
    ///   authority under the zero nullifier secret;
    /// - output 0: the delivery amount to the buyer's settlement destination;
    /// - output 1: the payment amount to the seller's settlement destination;
    /// - output 2: the delivery surplus to the seller's refund destination;
    /// - output 3: the payment surplus to the buyer's refund destination.
    ///
    /// A change output without surplus is a zero-amount UTXO, owned by the
    /// refund destination with the leg's mint, not a dummy (D2). The output
    /// blindings and the private tx blinding derive from the settlement seed
    /// and the DvP UTXO's nullifier, the transaction's first nullifier.
    pub fn derive(&self) -> Result<SettleDvpDerivation> {
        let terms = &self.terms;
        terms.validate()?;
        ensure!(&self.dvp_utxo.terms == terms, "DvP UTXO of other terms");
        ensure!(
            self.settlement_authority == terms.settlement_authority,
            "the signer is not the settlement authority"
        );
        let current_timestamp = u64::try_from(self.current_timestamp)
            .map_err(|_| anyhow!("negative timestamp (D5)"))?;
        ensure!(
            self.current_timestamp <= terms.expiry_timestamp,
            "the DvP has expired"
        );
        ensure!(
            terms
                .earliest_settlement_timestamp
                .is_none_or(|earliest| earliest <= self.current_timestamp),
            "settlement before the earliest settlement timestamp"
        );
        let delivery_change = leg_change(terms, &self.delivery_deposit, Leg::Delivery)?;
        let payment_change = leg_change(terms, &self.payment_deposit, Leg::Payment)?;

        let inputs = [
            self.dvp_utxo.to_input_utxo(self.dvp_utxo_leaf_index)?,
            self.delivery_deposit.to_input_utxo()?,
            self.payment_deposit.to_input_utxo()?,
        ];
        let [dvp_input, delivery_input, payment_input] = &inputs;
        let first_nullifier = dvp_input.nullifier;
        let blinding_seed = terms.settle_blinding_seed()?;

        let delivery_mint = self.delivery_deposit.mint;
        let payment_mint = self.payment_deposit.mint;
        let output = |mint: Mint, amount: u64, destination: ShieldedAddress| {
            SppProofOutputUtxo::new(mint, amount, destination).map_err(err)
        };
        let mut outputs = [
            output(
                delivery_mint,
                terms.delivery_amount,
                terms.buyer_settlement_destination,
            )?,
            output(
                payment_mint,
                terms.payment_amount,
                terms.seller_settlement_destination,
            )?,
            output(
                delivery_mint,
                delivery_change,
                terms.seller_refund_destination,
            )?,
            output(payment_mint, payment_change, terms.buyer_refund_destination)?,
        ];
        assign_output_blindings(&mut outputs, &first_nullifier, &blinding_seed)?;
        let private_tx_blinding =
            derive_private_tx_blinding(&first_nullifier, &blinding_seed).map_err(err)?;

        let spent = |input: &SppProofInputUtxo| ProofInputUtxo::try_from(input).map_err(err);
        let dvp_utxo = spent(dvp_input)?;
        let delivery_deposit = spent(delivery_input)?;
        let payment_deposit = spent(payment_input)?;
        let created = |output: &SppProofOutputUtxo| {
            ProofInputUtxo::try_from((output, self.output_tree_id)).map_err(err)
        };
        let [buyer_settlement, seller_settlement, seller_refund, buyer_refund] = &outputs;
        let buyer_settlement = created(buyer_settlement)?;
        let seller_settlement = created(seller_settlement)?;
        let seller_refund = created(seller_refund)?;
        let buyer_refund = created(buyer_refund)?;

        let private_tx_hash = PrivateTxHash::new(
            &[
                dvp_utxo.hash().map_err(err)?,
                delivery_deposit.hash().map_err(err)?,
                payment_deposit.hash().map_err(err)?,
            ],
            &[
                buyer_settlement.hash().map_err(err)?,
                seller_settlement.hash().map_err(err)?,
                seller_refund.hash().map_err(err)?,
                buyer_refund.hash().map_err(err)?,
            ],
            &private_tx_blinding,
        )
        .hash()
        .map_err(err)?;

        let creator_id = solana_owner_identity(terms.creator.as_array()).map_err(err)?;
        let settlement_authority_id =
            solana_owner_identity(self.settlement_authority.as_array()).map_err(err)?;
        let dvp_id = terms.dvp_id()?;
        let (dvp_authority, _) = terms.dvp_authority()?;
        let owner_hash = dvp_authority_owner_hash(&dvp_authority).map_err(err)?;
        let public_input_hash = SettleDvpPublicInput {
            private_tx_hash: &private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &owner_hash,
            settlement_authority_id: &settlement_authority_id,
            current_timestamp: self.current_timestamp,
        }
        .hash()
        .map_err(err)?;

        Ok(SettleDvpDerivation {
            inputs,
            outputs,
            blinding_seed,
            proof_inputs: SettleDvpProofInputs {
                public_input_hash,
                private_tx_hash,
                creator_id,
                dvp_id,
                dvp_authority_owner_hash: owner_hash,
                settlement_authority_id,
                current_timestamp,
                terms: SwapDvpProofInput::try_from(terms)?,
                dvp_utxo,
                delivery_deposit,
                payment_deposit,
                buyer_settlement,
                seller_settlement,
                seller_refund,
                buyer_refund,
                private_tx_blinding,
            },
        })
    }

    /// The settle witness of [`Self::derive`].
    pub fn to_proof_inputs(&self) -> Result<SettleDvpProofInputs> {
        Ok(self.derive()?.proof_inputs)
    }
}

/// Checks `deposit` funds `leg` of `terms` with at least the leg amount and
/// returns the surplus.
fn leg_change(terms: &SwapDvp, deposit: &Deposit, leg: Leg) -> Result<u64> {
    deposit.check_leg(terms, leg)?;
    deposit
        .amount
        .checked_sub(leg.amount(terms))
        .ok_or_else(|| anyhow!("{leg:?} deposit is below the leg amount"))
}

use anyhow::{bail, ensure, Result};
use dvp_program::{
    instructions::{abort_dvp::AbortDvpPublicInput, shared::dvp_authority_owner_hash},
    ABORT_MODE_CANCEL, ABORT_MODE_REJECT,
};
use dvp_prover::{AbortDvpProofInputs, ProofInputUtxo, SwapDvpProofInput};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_program::{derive_private_tx_blinding, PrivateTxHash};
use zolana_transaction::{utxo::SppProofInputUtxo, Mint, SppProofOutputUtxo};

use crate::{
    err,
    shared::assign_output_blindings,
    state::{Deposit, DvpUtxo, Leg, SwapDvp},
};

/// Which instruction aborts the DvP; the abort proof's public `mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortMode {
    /// cancel_dvp: the settlement authority aborts.
    Cancel,
    /// reject_dvp: the seller or the buyer aborts.
    Reject,
}

impl AbortMode {
    /// `ABORT_MODE_CANCEL` or `ABORT_MODE_REJECT`, as the circuit takes it.
    pub fn value(self) -> u64 {
        u64::from(match self {
            Self::Cancel => ABORT_MODE_CANCEL,
            Self::Reject => ABORT_MODE_REJECT,
        })
    }
}

/// Everything the abort proof needs. The 3x2 transaction spends the DvP
/// UTXO and whichever deposits exist and refunds each leg to its funder's
/// refund destination; every blinding derives from the terms' abort seed.
pub struct AbortDvpProofInputParams {
    pub terms: SwapDvp,
    /// The signer: the settlement authority to cancel, the seller or the
    /// buyer to reject.
    pub signer: Address,
    pub mode: AbortMode,
    pub dvp_utxo: DvpUtxo,
    /// Leaf index of the DvP UTXO in its tree (`DiscoveredDvp::leaf_index`).
    pub dvp_utxo_leaf_index: u64,
    /// The delivery and payment mints with their pool asset ids. A leg
    /// without a deposit still gets a zero-amount refund in its mint (D2),
    /// whose ciphertext carries the asset id.
    pub delivery_mint: Mint,
    pub payment_mint: Mint,
    /// The delivery deposit, refunded in full, if the seller funded the leg.
    pub delivery_deposit: Option<Deposit>,
    /// The payment deposit, refunded in full, if the buyer funded the leg.
    pub payment_deposit: Option<Deposit>,
    /// Raw id of the tree the random padding inputs are spent from; use the
    /// real inputs' tree.
    pub padding_tree_id: u16,
    /// Raw id of the tree the two refunds are appended to.
    pub output_tree_id: u16,
}

/// The abort transaction as both proofs see it: the SPP inputs and outputs
/// (owner addresses and derived blindings set, ready to encrypt) and the
/// abort witness over the same values.
pub struct AbortDvpDerivation {
    /// `[DvP UTXO, slot 1, slot 2]`: the deposits first, random padding
    /// (`SppProofInputUtxo::dummy`) last (D3).
    pub inputs: [SppProofInputUtxo; 3],
    /// `[seller refund, buyer refund]`.
    pub outputs: [SppProofOutputUtxo; 2],
    /// `Poseidon(ABORT_TAG, terms.blinding)`.
    pub blinding_seed: [u8; 32],
    pub proof_inputs: AbortDvpProofInputs,
}

impl AbortDvpProofInputParams {
    /// Checks the signer against the mode and the deposits against their
    /// legs, then derives the transaction:
    ///
    /// - input 0: the DvP UTXO;
    /// - inputs 1 and 2: the deposits, padding last (D3). With only the
    ///   payment deposit, it fills slot 1 and the leg-order bit
    ///   `payment_first` is set;
    /// - output 0: the delivery deposit (or 0) to the seller's refund
    ///   destination;
    /// - output 1: the payment deposit (or 0) to the buyer's refund
    ///   destination.
    ///
    /// Padding is random padding, not compact padding, so the published
    /// nullifiers do not reveal how many legs were funded. Its hash
    /// contributes 0 to the private tx hash, as SPP computes it for a dummy.
    pub fn derive(&self) -> Result<AbortDvpDerivation> {
        let terms = &self.terms;
        terms.validate()?;
        ensure!(&self.dvp_utxo.terms == terms, "DvP UTXO of other terms");
        ensure!(
            self.delivery_mint.asset == terms.delivery_mint,
            "delivery mint is not the terms' mint"
        );
        ensure!(
            self.payment_mint.asset == terms.payment_mint,
            "payment mint is not the terms' mint"
        );
        // The rejecting party's leg bit: the seller is 0, the buyer 1. In
        // cancel mode the circuit ignores it.
        let leg = match self.mode {
            AbortMode::Cancel => {
                ensure!(
                    self.signer == terms.settlement_authority,
                    "only the settlement authority cancels"
                );
                false
            }
            AbortMode::Reject if self.signer == terms.seller => false,
            AbortMode::Reject if self.signer == terms.buyer => true,
            AbortMode::Reject => bail!("only the seller or the buyer rejects"),
        };
        let delivery = self.checked_deposit(self.delivery_deposit.as_ref(), Leg::Delivery)?;
        let payment = self.checked_deposit(self.payment_deposit.as_ref(), Leg::Payment)?;

        let payment_first = delivery.is_none() && payment.is_some();
        let mut deposits = delivery.into_iter().chain(payment);
        let (slot1_deposit, slot2_deposit) = (deposits.next(), deposits.next());
        let (slot1_input, slot1, slot1_hash) = self.slot(slot1_deposit)?;
        let (slot2_input, slot2, slot2_hash) = self.slot(slot2_deposit)?;

        let dvp_input = self.dvp_utxo.to_input_utxo(self.dvp_utxo_leaf_index)?;
        let first_nullifier = dvp_input.nullifier;
        let blinding_seed = terms.abort_blinding_seed()?;

        let refund = |mint: Mint, deposit: Option<&Deposit>, destination| {
            let amount = deposit.map_or(0, |deposit| deposit.amount);
            SppProofOutputUtxo::new(mint, amount, destination).map_err(err)
        };
        let mut outputs = [
            refund(
                self.delivery_mint,
                delivery,
                terms.seller_refund_destination,
            )?,
            refund(self.payment_mint, payment, terms.buyer_refund_destination)?,
        ];
        assign_output_blindings(&mut outputs, &first_nullifier, &blinding_seed)?;
        let private_tx_blinding =
            derive_private_tx_blinding(&first_nullifier, &blinding_seed).map_err(err)?;

        let dvp_utxo = ProofInputUtxo::try_from(&dvp_input).map_err(err)?;
        let created = |output: &SppProofOutputUtxo| {
            ProofInputUtxo::try_from((output, self.output_tree_id)).map_err(err)
        };
        let [seller_refund, buyer_refund] = &outputs;
        let seller_refund = created(seller_refund)?;
        let buyer_refund = created(buyer_refund)?;

        let private_tx_hash = PrivateTxHash::new(
            &[dvp_utxo.hash().map_err(err)?, slot1_hash, slot2_hash],
            &[
                seller_refund.hash().map_err(err)?,
                buyer_refund.hash().map_err(err)?,
            ],
            &private_tx_blinding,
        )
        .hash()
        .map_err(err)?;

        let creator_id = solana_owner_identity(terms.creator.as_array()).map_err(err)?;
        let signer_id = solana_owner_identity(self.signer.as_array()).map_err(err)?;
        let dvp_id = terms.dvp_id()?;
        let (dvp_authority, _) = terms.dvp_authority()?;
        let owner_hash = dvp_authority_owner_hash(&dvp_authority).map_err(err)?;
        let mode = self.mode.value();
        let public_input_hash = AbortDvpPublicInput {
            private_tx_hash: &private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &owner_hash,
            signer_id: &signer_id,
            mode,
        }
        .hash()
        .map_err(err)?;

        Ok(AbortDvpDerivation {
            inputs: [dvp_input, slot1_input, slot2_input],
            outputs,
            blinding_seed,
            proof_inputs: AbortDvpProofInputs {
                public_input_hash,
                private_tx_hash,
                creator_id,
                dvp_id,
                dvp_authority_owner_hash: owner_hash,
                signer_id,
                mode,
                terms: SwapDvpProofInput::try_from(terms)?,
                leg,
                payment_first,
                padding1: slot1_deposit.is_none(),
                padding2: slot2_deposit.is_none(),
                dvp_utxo,
                slot1,
                slot2,
                seller_refund,
                buyer_refund,
                private_tx_blinding,
            },
        })
    }

    /// The abort witness of [`Self::derive`].
    pub fn to_proof_inputs(&self) -> Result<AbortDvpProofInputs> {
        Ok(self.derive()?.proof_inputs)
    }

    /// Checks `deposit`, if any, funds `leg` of the terms in the leg's mint.
    /// Any amount is refunded in full.
    fn checked_deposit<'a>(
        &self,
        deposit: Option<&'a Deposit>,
        leg: Leg,
    ) -> Result<Option<&'a Deposit>> {
        let Some(deposit) = deposit else {
            return Ok(None);
        };
        let mint = match leg {
            Leg::Delivery => self.delivery_mint,
            Leg::Payment => self.payment_mint,
        };
        ensure!(
            deposit.terms == self.terms,
            "{leg:?} deposit of other terms"
        );
        ensure!(deposit.leg == leg, "{leg:?} deposit funds the other leg");
        ensure!(
            deposit.mint == mint,
            "{leg:?} deposit mint is not the leg's mint"
        );
        Ok(Some(deposit))
    }

    /// One deposit slot: the SPP input, the abort witness and the slot's
    /// private tx hash contribution. Padding is a random dummy, as SPP
    /// counts it: hash 0. Its witness is unconstrained (the circuit counts
    /// neither its hash nor its amount), so it is all zero.
    fn slot(
        &self,
        deposit: Option<&Deposit>,
    ) -> Result<(SppProofInputUtxo, ProofInputUtxo, [u8; 32])> {
        let Some(deposit) = deposit else {
            let dummy = SppProofInputUtxo::dummy(self.padding_tree_id).map_err(err)?;
            return Ok((dummy, ProofInputUtxo::default(), [0u8; 32]));
        };
        let input = deposit.to_input_utxo()?;
        let witness = ProofInputUtxo::try_from(&input).map_err(err)?;
        let hash = witness.hash().map_err(err)?;
        Ok((input, witness, hash))
    }
}

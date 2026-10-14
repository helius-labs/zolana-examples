use anyhow::{ensure, Result};
use dvp_program::instructions::{
    reclaim_dvp::ReclaimDvpPublicInput, shared::dvp_authority_owner_hash,
};
use dvp_prover::{ProofInputUtxo, ReclaimDvpProofInputs, SwapDvpProofInput};
use solana_address::Address;
use zolana_hasher::primitives::solana_owner_identity;
use zolana_program::PrivateTxHash;
use zolana_transaction::instructions::transact::SppProofOutputUtxo;

use crate::{
    err,
    state::{Deposit, Leg, SwapDvp},
};

/// Everything the reclaim proof needs. The 1x2 transaction spends `deposit`
/// in input 0 and writes `refund` in output 0; output 1 is compact padding.
/// The blindings are the caller's (random, not derived from the terms).
pub struct ReclaimDvpProofInputParams {
    pub terms: SwapDvp,
    /// The reclaiming party: the seller for the delivery leg, the buyer for
    /// the payment leg.
    pub signer: Address,
    pub deposit: Deposit,
    /// The deposit's asset and full amount, with no data, to any owner.
    pub refund: SppProofOutputUtxo,
    /// Raw id of the tree the refund is appended to.
    pub refund_tree_id: u16,
    pub private_tx_blinding: [u8; 32],
}

impl ReclaimDvpProofInputParams {
    /// Checks that the signer funds the deposit's leg and that the refund
    /// returns the whole deposit, then derives the deposit and refund
    /// commitments, `private_tx_hash` over `[deposit]` inputs and
    /// `[refund, 0]` outputs, and the public input hash.
    pub fn to_proof_inputs(&self) -> Result<ReclaimDvpProofInputs> {
        let terms = &self.terms;
        let deposit = &self.deposit;
        deposit.check_leg(terms, deposit.leg)?;
        let (party, leg) = match deposit.leg {
            Leg::Delivery => (terms.seller, false),
            Leg::Payment => (terms.buyer, true),
        };
        ensure!(
            self.signer == party,
            "the signer does not fund the deposit's leg"
        );
        let refund = &self.refund;
        ensure!(
            !refund.compact && !refund.is_dummy(),
            "refund is a padding output"
        );
        ensure!(
            refund.asset.asset == deposit.mint.asset,
            "refund asset is not the deposit asset"
        );
        ensure!(
            refund.amount == deposit.amount,
            "refund amount is not the deposit amount"
        );
        ensure!(
            refund.data_hash.unwrap_or_default() == [0u8; 32],
            "refund carries a data hash"
        );

        let creator_id = solana_owner_identity(terms.creator.as_array()).map_err(err)?;
        let signer_id = solana_owner_identity(self.signer.as_array()).map_err(err)?;
        let dvp_id = terms.dvp_id()?;
        let (dvp_authority, _) = terms.dvp_authority()?;
        let owner_hash = dvp_authority_owner_hash(&dvp_authority).map_err(err)?;

        let deposit_utxo = ProofInputUtxo::try_from(&deposit.to_input_utxo()?).map_err(err)?;
        let refund_utxo = ProofInputUtxo::try_from((refund, self.refund_tree_id)).map_err(err)?;
        let private_tx_hash = PrivateTxHash::new(
            &[deposit_utxo.hash().map_err(err)?],
            &[refund_utxo.hash().map_err(err)?, [0u8; 32]],
            &self.private_tx_blinding,
        )
        .hash()
        .map_err(err)?;
        let public_input_hash = ReclaimDvpPublicInput {
            private_tx_hash: &private_tx_hash,
            creator_id: &creator_id,
            dvp_id: &dvp_id,
            dvp_authority_owner_hash: &owner_hash,
            signer_id: &signer_id,
        }
        .hash()
        .map_err(err)?;

        Ok(ReclaimDvpProofInputs {
            public_input_hash,
            private_tx_hash,
            creator_id,
            dvp_id,
            dvp_authority_owner_hash: owner_hash,
            signer_id,
            terms: SwapDvpProofInput::try_from(terms)?,
            leg,
            deposit: deposit_utxo,
            refund: refund_utxo,
            private_tx_blinding: self.private_tx_blinding,
        })
    }
}

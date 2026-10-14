use anyhow::Result;
use dvp_prover::AbortDvpProofInputs;
use solana_address::Address;
use solana_instruction::Instruction;
use zolana_client::ZolanaIndexer;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{instructions::transact::SppProofInputs, Mint};

use super::{AbortDvpDerivation, AbortDvpProofInputParams, AbortMode};
use crate::{
    encryption::encrypt_outputs,
    err,
    instructions::{cancel_dvp::CancelDvp, reject_dvp::RejectDvp},
    shared::{ensure_output_tree, prove_pda_transact},
    state::{Deposit, DvpUtxo, SwapDvp},
};

/// Everything the abort transaction needs besides the signer's keys and the
/// prover.
pub struct AbortDvpTransactionParams {
    pub terms: SwapDvp,
    /// Cancel (the settlement authority signs) or reject (a party signs).
    pub mode: AbortMode,
    pub dvp_utxo: DvpUtxo,
    /// Leaf index of the DvP UTXO in its tree (`DiscoveredDvp::leaf_index`).
    pub dvp_utxo_leaf_index: u64,
    /// The delivery and payment mints with their pool asset ids; a leg
    /// without a deposit still gets a zero-amount refund in its mint (D2).
    pub delivery_mint: Mint,
    pub payment_mint: Mint,
    /// The deposits the signer knows of. Each one given is refunded in full;
    /// a missing leg becomes random padding and a zero-amount refund.
    pub delivery_deposit: Option<Deposit>,
    pub payment_deposit: Option<Deposit>,
    /// Pays the transaction and is the SPP payer, the first signer the SPP
    /// proof binds.
    pub payer: Address,
    /// Raw id of the tree the refunds are appended to and the padding
    /// inputs are spent from.
    pub tree_id: u16,
}

/// The abort transaction before its proofs: the SPP proof inputs and the
/// abort proof inputs over the same transaction.
pub struct AbortDvpTransaction {
    mode: AbortMode,
    signer: Address,
    creator: Address,
    bump: u8,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: AbortDvpProofInputs,
}

/// The proven abort, as the instruction its mode calls for.
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
    /// Builds the abort `transact` at SPP shape 3x2 from
    /// [`AbortDvpProofInputParams::derive`]: the DvP UTXO and the known
    /// deposits in (padding last, D3), the seller's and the buyer's refunds
    /// out, every blinding derived from the terms' abort seed.
    ///
    /// Both refunds are encrypted to their refund destinations, a
    /// zero-amount refund too. The transaction viewing key is derived from
    /// the signer's viewing key and the DvP UTXO's nullifier, the first
    /// nullifier.
    ///
    /// Errors unless `signer` may abort in `mode` (cancel: the settlement
    /// authority; reject: the seller or the buyer), each deposit funds its
    /// leg of the terms, and the DvP UTXO and the deposits sit in the tree
    /// with id `tree_id`, which the instruction reads its inputs from.
    pub fn build(self, signer: &ShieldedKeypair) -> Result<AbortDvpTransaction> {
        let Self {
            terms,
            mode,
            dvp_utxo,
            dvp_utxo_leaf_index,
            delivery_mint,
            payment_mint,
            delivery_deposit,
            payment_deposit,
            payer,
            tree_id,
        } = self;
        ensure_output_tree("the DvP UTXO", dvp_utxo.tree_id, tree_id)?;
        for deposit in delivery_deposit.iter().chain(payment_deposit.iter()) {
            ensure_output_tree("a deposit", deposit.tree_id, tree_id)?;
        }
        let signer_address = signer
            .shielded_address()
            .map_err(err)?
            .solana_address()
            .map_err(err)?;
        let creator = terms.creator;
        let (_, bump) = terms.dvp_authority()?;
        let AbortDvpDerivation {
            inputs,
            outputs,
            blinding_seed,
            proof_inputs,
        } = AbortDvpProofInputParams {
            terms,
            signer: signer_address,
            mode,
            dvp_utxo,
            dvp_utxo_leaf_index,
            delivery_mint,
            payment_mint,
            delivery_deposit,
            payment_deposit,
            padding_tree_id: tree_id,
            output_tree_id: tree_id,
        }
        .derive()?;

        let [dvp_input, ..] = &inputs;
        let tx_viewing_key = signer
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
        Ok(AbortDvpTransaction {
            mode,
            signer: signer_address,
            creator,
            bump,
            spp_proof_inputs,
            proof_inputs,
        })
    }
}

impl AbortDvpTransaction {
    /// Proves the SPP transaction through `indexer` (which serves the
    /// inclusion proofs of the DvP UTXO and the deposits, and the
    /// non-inclusion proofs of the padding nullifiers) and the abort proof
    /// locally, and returns the cancel or reject builder for `tree`, the
    /// account of the tree with id `tree_id`.
    pub fn prove(self, indexer: &ZolanaIndexer, tree: Address) -> Result<AbortDvp> {
        let Self {
            mode,
            signer,
            creator,
            bump,
            spp_proof_inputs,
            proof_inputs,
        } = self;
        let payer = spp_proof_inputs.payer;
        let spp = prove_pda_transact(indexer, spp_proof_inputs, &proof_inputs.private_tx_hash)?;
        let abort_proof = proof_inputs.prove().map_err(err)?.into();
        let dvp_id = proof_inputs.dvp_id;
        Ok(match mode {
            AbortMode::Cancel => AbortDvp::Cancel(CancelDvp {
                payer,
                settlement_authority: signer,
                tree,
                creator,
                dvp_id,
                bump,
                cancel_proof: abort_proof,
                spp,
            }),
            AbortMode::Reject => AbortDvp::Reject(RejectDvp {
                payer,
                signer,
                tree,
                creator,
                dvp_id,
                bump,
                reject_proof: abort_proof,
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
        FIXTURE_LEAF_INDEX, FIXTURE_SURPLUS,
    };

    /// Leaf the fixture abort appends its first output at.
    const ABORT_LEAF: u64 = 40;

    fn params(
        fixture: &DepositFixture,
        mode: AbortMode,
        delivery: bool,
        payment: bool,
    ) -> AbortDvpTransactionParams {
        let [delivery_deposit, payment_deposit] = &fixture.deposits;
        AbortDvpTransactionParams {
            terms: fixture.terms.clone(),
            mode,
            dvp_utxo: DvpUtxo::new(fixture.terms.clone(), delivery_deposit.tree_id),
            dvp_utxo_leaf_index: FIXTURE_LEAF_INDEX,
            delivery_mint: delivery_deposit.mint,
            payment_mint: payment_deposit.mint,
            delivery_deposit: delivery.then(|| delivery_deposit.clone()),
            payment_deposit: payment.then(|| payment_deposit.clone()),
            payer: Address::new_from_array([7; 32]),
            tree_id: delivery_deposit.tree_id,
        }
    }

    /// The seller's delivery refunds and the buyer's payment refunds that
    /// `transaction` pays, plus its input and encrypted output counts.
    fn refunds(
        fixture: &DepositFixture,
        transaction: &AbortDvpTransaction,
    ) -> (usize, usize, Vec<u64>, Vec<u64>) {
        let [seller, buyer] = &fixture.funders;
        let terms = &fixture.terms;
        let inputs = &transaction.spp_proof_inputs;
        let tx = shielded_transaction(inputs, ABORT_LEAF);
        (
            inputs.input_utxos.len(),
            inputs.external_data.outputs.len(),
            spendable_amounts(seller, &tx, &fixture.registry, &terms.delivery_mint),
            spendable_amounts(buyer, &tx, &fixture.registry, &terms.payment_mint),
        )
    }

    #[test]
    fn cancel_refunds_both_deposits_in_full() {
        let fixture = deposit_fixture();
        let terms = &fixture.terms;
        let transaction = params(&fixture, AbortMode::Cancel, true, true)
            .build(&fixture.authority)
            .expect("build");
        assert_eq!(
            (
                refunds(&fixture, &transaction),
                transaction
                    .spp_proof_inputs
                    .private_tx_blinding()
                    .expect("blinding"),
            ),
            (
                (
                    3,
                    2,
                    vec![terms.delivery_amount + FIXTURE_SURPLUS],
                    vec![terms.payment_amount + FIXTURE_SURPLUS],
                ),
                transaction.proof_inputs.private_tx_blinding,
            )
        );
    }

    #[test]
    fn buyer_reject_with_only_the_payment_deposit_refunds_zero_delivery() {
        let fixture = deposit_fixture();
        let [_, buyer] = &fixture.funders;
        let transaction = params(&fixture, AbortMode::Reject, false, true)
            .build(buyer)
            .expect("build");
        let dummies: Vec<bool> = transaction
            .spp_proof_inputs
            .input_utxos
            .iter()
            .map(|input| input.is_dummy())
            .collect();
        assert_eq!(
            (
                refunds(&fixture, &transaction),
                dummies,
                transaction.proof_inputs.payment_first,
            ),
            (
                (
                    3,
                    2,
                    vec![0],
                    vec![fixture.terms.payment_amount + FIXTURE_SURPLUS],
                ),
                vec![false, false, true],
                true,
            )
        );
    }

    #[test]
    fn build_refuses_a_signer_the_mode_does_not_allow() {
        let fixture = deposit_fixture();
        let [seller, buyer] = &fixture.funders;
        assert!(params(&fixture, AbortMode::Cancel, true, true)
            .build(seller)
            .is_err());
        assert!(params(&fixture, AbortMode::Cancel, true, true)
            .build(buyer)
            .is_err());
        assert!(params(&fixture, AbortMode::Reject, true, true)
            .build(&fixture.authority)
            .is_err());
    }
}

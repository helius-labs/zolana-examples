use anyhow::{anyhow, ensure, Result};
use solana_address::Address;
use spr_client::SprIndexer;
use spr_keypair::ShieldedKeypair;
use spr_program::{ProgramProofInputs, TransactionOptions};
use spr_transaction::{instructions::transact::SppProofInputs, WalletUtxo};
use swap_program::circuits::{TakeClient, TakePrivateInputsClient, TakePublicInputsClient};

use super::{take_blinding_seed, Take};
use crate::{
    err,
    proof::{self, CircuitId},
    shared::{field, identity, prove_transact, TakeAuthority},
    state::OrderUtxo,
};

/// Output slot of the source paid to the taker in the take transaction.
pub const SOURCE_OUTPUT_SLOT: usize = 0;
/// Output slot of the destination paid to the maker in the take transaction.
pub const DESTINATION_OUTPUT_SLOT: usize = 1;

pub struct TakeTransactionParams {
    pub order_utxo: OrderUtxo,
    /// The order UTXO's leaf index in the output tree.
    pub order_leaf_index: u64,
    /// The taker's UTXO of exactly `destination_amount` of the destination
    /// mint, in the output tree.
    pub taker_input: WalletUtxo,
    pub payer: Address,
    /// The output tree, which also holds both inputs.
    pub tree_id: u16,
}

pub struct TakeTransaction {
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ProgramProofInputs,
    /// The order UTXO's nullifier, the transaction's first nullifier.
    pub first_nullifier: [u8; 32],
    /// The source output's blinding.
    pub source_output_blinding: [u8; 32],
    /// The maker payout's blinding, which the maker derives with
    /// [`OrderUtxo::derived_destination_output`].
    pub destination_output_blinding: [u8; 32],
}

impl TakeTransactionParams {
    /// 1. Check the taker input pays the destination and the taker may take.
    /// 2. Build the program transaction under the order's take blinding seed,
    ///    with the order expiry as the transact expiry.
    /// 3. Check the maker can derive its payout from the order opening.
    /// 4. Encrypt the outputs.
    pub fn build(self, taker: &ShieldedKeypair) -> Result<TakeTransaction> {
        // 1. Check the taker input and the designated taker.
        let sender = taker.shielded_address().map_err(err)?;
        let terms = &self.order_utxo.terms;
        ensure!(
            self.taker_input.utxo.asset == terms.destination_mint
                && self.taker_input.utxo.amount == terms.destination_amount,
            "the taker input must hold exactly the destination amount of the destination mint"
        );
        ensure!(
            self.taker_input.tree_id == self.tree_id,
            "the taker input is in tree {}, not in the output tree {}",
            self.taker_input.tree_id,
            self.tree_id
        );
        if let Some(designated) = terms.taker {
            ensure!(
                identity(&sender.solana_address().map_err(err)?)? == designated,
                "the order designates another taker"
            );
        }
        let order_input = self
            .order_utxo
            .to_input_utxo(self.tree_id, self.order_leaf_index)?;
        let first_nullifier = order_input.nullifier;

        // 2. Build the program transaction.
        let transaction = TakeClient {
            private: TakePrivateInputsClient {
                tx_context: TransactionOptions {
                    blinding_seed: Some(take_blinding_seed(&self.order_utxo.blinding)?),
                    output_tree_id: Some(self.tree_id),
                },
                terms: terms.circuit_terms()?,
                order_utxo: order_input,
                taker_in: self.taker_input,
                maker_address: terms.maker,
            },
            public: TakePublicInputsClient {
                expiry: terms.expiry,
                first_nullifier: field(first_nullifier)?,
            },
        }
        .prepare(&sender, self.payer)
        .map_err(err)?;
        let finalized = transaction.finalized.with_expiry_unix_ts(terms.expiry);
        ensure!(
            finalized.first_nullifier().map_err(err)? == first_nullifier,
            "the order UTXO must be the first input"
        );

        // 3. Check the maker can derive its payout.
        let outputs = finalized.output_utxos();
        let source_output_blinding = outputs
            .get(SOURCE_OUTPUT_SLOT)
            .ok_or_else(|| anyhow!("the take transaction has no source output"))?
            .blinding;
        let destination = outputs
            .get(DESTINATION_OUTPUT_SLOT)
            .ok_or_else(|| anyhow!("the take transaction has no destination output"))?;
        ensure!(
            destination.hash(self.tree_id).map_err(err)?
                == self
                    .order_utxo
                    .derived_destination_output(&first_nullifier)?
                    .hash(self.tree_id)
                    .map_err(err)?,
            "the maker payout does not derive from the order opening"
        );
        let destination_output_blinding = destination.blinding;

        // 4. Encrypt the outputs.
        let key = taker
            .get_transaction_viewing_key(&first_nullifier)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(TakeTransaction {
            spp_proof_inputs,
            proof_inputs: transaction.proof_inputs,
            first_nullifier,
            source_output_blinding,
            destination_output_blinding,
        })
    }
}

impl TakeTransaction {
    /// 1. Prove the SPP transaction with the order-authority and taker keys.
    /// 2. Prove the take circuit and build the instruction.
    pub fn prove(
        self,
        indexer: &SprIndexer,
        tree: Address,
        taker: &ShieldedKeypair,
    ) -> Result<Take> {
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp_proof = prove_transact(indexer, self.spp_proof_inputs, &TakeAuthority(taker))?;
        // 2. Prove the take circuit and build the instruction.
        let take_proof = proof::prove(CircuitId::Take, &self.proof_inputs)?;
        Ok(Take {
            payer,
            tree,
            take_proof,
            spp_proof,
        })
    }
}

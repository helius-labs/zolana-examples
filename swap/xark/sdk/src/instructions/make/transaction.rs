use anyhow::{anyhow, ensure, Result};
use solana_address::Address;
use spr_client::SprIndexer;
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_program::{ProgramOwner, ProgramProofInputs, TransactionOptions};
use spr_transaction::{instructions::transact::SppProofInputs, WalletUtxo};
use swap_program::circuits::{MakeClient, MakePrivateInputsClient, MakePublicInputsClient};

use super::{Make, OrderMarker};
use crate::{
    err, order_authority,
    proof::{self, CircuitId},
    shared::prove_transact,
    state::{OrderTerms, OrderUtxo},
};

/// Output slot of the change in the make transaction.
pub const CHANGE_SLOT: usize = 0;
/// Output slot of the order UTXO in the make transaction; the program reads
/// the order UTXO hash for the marker from it.
pub const ORDER_SLOT: usize = 1;

pub struct MakeTransactionParams {
    pub terms: OrderTerms,
    /// The amount locked in the order UTXO; the rest of `source` is change.
    pub source_amount: u64,
    /// The spent source UTXO. The transaction is signed by the program alone,
    /// so it must be owned by the order-authority PDA, as a proofless deposit
    /// to [`crate::order_authority`]'s address is.
    pub source: WalletUtxo,
    /// The taker the order output is encrypted to and the marker is tagged
    /// for. It may differ from `terms.taker`, the identity allowed to take.
    pub taker_address: ShieldedAddress,
    pub payer: Address,
    /// The output tree, which also holds `source`.
    pub tree_id: u16,
}

pub struct MakeTransaction {
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ProgramProofInputs,
    /// The order UTXO the transaction creates, with its derived blinding.
    pub order_utxo: OrderUtxo,
    /// The change output's blinding.
    pub change_blinding: [u8; 32],
}

impl MakeTransactionParams {
    /// 1. Check the terms name the maker and the source sits in the output
    ///    tree.
    /// 2. Build the program transaction.
    /// 3. Read the order UTXO and the change it created.
    /// 4. Encrypt the outputs and add the taker's marker message.
    pub fn build(self, maker: &ShieldedKeypair) -> Result<MakeTransaction> {
        // 1. Check the terms name the maker and the source tree.
        let sender = maker.shielded_address().map_err(err)?;
        ensure!(
            self.terms.maker == sender,
            "the order terms name another maker"
        );
        ensure!(
            self.source.tree_id == self.tree_id,
            "the source is in tree {}, not in the output tree {}",
            self.source.tree_id,
            self.tree_id
        );
        let source_mint = self.source.utxo.asset;

        // 2. Build the program transaction.
        let transaction = MakeClient {
            private: MakePrivateInputsClient {
                tx_context: TransactionOptions {
                    blinding_seed: None,
                    output_tree_id: Some(self.tree_id),
                },
                terms: self.terms.circuit_terms()?,
                source_amount: self.source_amount,
                source: self.source,
                order_address: order_authority()
                    .address(self.taker_address.viewing_pubkey)
                    .map_err(err)?,
            },
            public: MakePublicInputsClient {},
        }
        .prepare(&sender, self.payer)
        .map_err(err)?;
        let finalized = transaction.finalized;

        // 3. Read the order UTXO and the change it created.
        let outputs = finalized.output_utxos();
        let change = outputs
            .get(CHANGE_SLOT)
            .ok_or_else(|| anyhow!("the make transaction has no change output"))?;
        let order_output = outputs
            .get(ORDER_SLOT)
            .ok_or_else(|| anyhow!("the make transaction has no order output"))?;
        let order_utxo = OrderUtxo {
            terms: self.terms,
            blinding: order_output.blinding,
            source_mint,
            source_amount: self.source_amount,
        };
        let order_utxo_hash = order_output.hash(self.tree_id).map_err(err)?;
        ensure!(
            order_utxo.hash(self.tree_id)? == order_utxo_hash,
            "the order output does not commit the order terms"
        );
        let change_blinding = change.blinding;

        // 4. Encrypt the outputs and add the marker. The program overwrites
        // its data with the same `MarkerData`.
        let key = maker
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let mut spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        spp_proof_inputs.external_data.messages.push(
            OrderMarker {
                order_utxo_hash,
                maker_pubkey: self.payer,
                taker_address: self.taker_address,
            }
            .message()
            .map_err(err)?,
        );
        Ok(MakeTransaction {
            spp_proof_inputs,
            proof_inputs: transaction.proof_inputs,
            order_utxo,
            change_blinding,
        })
    }
}

impl MakeTransaction {
    /// 1. Prove the SPP transaction, whose real inputs the order-authority PDA
    ///    owns.
    /// 2. Prove the make circuit and build the instruction.
    pub fn prove(self, indexer: &SprIndexer, tree: Address) -> Result<Make> {
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp_proof = prove_transact(
            indexer,
            self.spp_proof_inputs,
            &ProgramOwner::nullifier_key(),
        )?;
        // 2. Prove the make circuit and build the instruction.
        let make_proof = proof::prove(CircuitId::Make, &self.proof_inputs)?;
        Ok(Make {
            payer,
            tree,
            make_proof,
            spp_proof,
        })
    }
}

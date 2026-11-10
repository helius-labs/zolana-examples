use anyhow::{anyhow, ensure, Result};
use solana_address::Address;
use spr_client::SprIndexer;
use spr_keypair::{ShieldedAddress, ShieldedKeypair};
use spr_program::{ProgramProofInputs, TransactionOptions};
use spr_transaction::{instructions::transact::SppProofInputs, WalletUtxo};
use swap_program::circuits::{MakeClient, MakePrivateInputsClient, MakePublicInputsClient};

use super::{Make, OrderMarker};
use crate::{
    err, order_authority,
    proof::{self, CircuitId},
    shared::{field, identity, prove_transact},
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
    /// The spent source UTXO, owned by the maker, who signs for it. The rest
    /// of it returns to the maker as change. Its tree is the output tree.
    pub source: WalletUtxo,
    /// The taker the order output is encrypted to and the marker is tagged
    /// for. It may differ from `terms.taker`, the identity allowed to take.
    pub taker_address: ShieldedAddress,
    /// The maker's Solana address, the fee payer and the signer the program
    /// writes into the marker and checks against the source owner.
    pub payer: Address,
}

pub struct MakeTransaction {
    /// The taker's viewing key, the order UTXO address's viewing key, which
    /// the program hashes into the make proof's public `order_owner`.
    pub viewing_pubkey: [u8; 33],
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ProgramProofInputs,
    /// The order UTXO the transaction creates, with its derived blinding.
    pub order_utxo: OrderUtxo,
    /// The change output's blinding.
    pub change_blinding: [u8; 32],
}

impl MakeTransactionParams {
    /// 1. Check the terms name the maker and the maker owns the source.
    /// 2. Build the program transaction.
    /// 3. Read the order UTXO and the change it created.
    /// 4. Encrypt the outputs and add the taker's marker message.
    pub fn build(self, maker: &ShieldedKeypair) -> Result<MakeTransaction> {
        // 1. Check the terms name the maker and the maker owns the source.
        let sender = maker.shielded_address().map_err(err)?;
        ensure!(
            self.terms.maker == sender,
            "the order terms name another maker"
        );
        ensure!(
            self.payer == sender.solana_address().map_err(err)?,
            "the payer is not the maker's Solana address"
        );
        ensure!(
            self.source.utxo.owner == maker.signing_pubkey(),
            "the source is not owned by the maker"
        );
        let source_mint = self.source.utxo.asset;
        let tree_id = self.source.tree_id;

        // 2. Build the program transaction. The circuit takes the maker from
        // the source owner; the order UTXO's owner is the maker's order
        // authority with the taker's viewing key, which the program derives
        // from the maker signer.
        let terms = self.terms.circuit_terms()?;
        let transaction = MakeClient {
            private: MakePrivateInputsClient {
                tx_context: TransactionOptions {
                    blinding_seed: None,
                    output_tree_id: Some(tree_id),
                },
                source: self.source,
                source_amount: self.source_amount,
                destination_mint: terms.destination_mint,
                destination_amount: terms.destination_amount,
                expiry: terms.expiry,
                taker_pk_fe: terms.taker_pk_fe,
            },
            public: MakePublicInputsClient {
                maker_owner_pk_field: field(identity(&self.payer)?)?,
                order_owner: order_authority(&self.payer)
                    .address(self.taker_address.viewing_pubkey)
                    .map_err(err)?,
            },
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
        let order_utxo_hash = order_output.hash(tree_id).map_err(err)?;
        ensure!(
            order_utxo.hash(tree_id)? == order_utxo_hash,
            "the order output does not commit the order terms"
        );
        let change_blinding = change.blinding;

        // 4. Encrypt the outputs and add the marker, the transaction's only
        // message. The program checks its data is the `MarkerData` of the
        // order output and the payer, and forwards it unchanged.
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
            viewing_pubkey: *self.taker_address.viewing_pubkey.as_bytes(),
            spp_proof_inputs,
            proof_inputs: transaction.proof_inputs,
            order_utxo,
            change_blinding,
        })
    }
}

impl MakeTransaction {
    /// 1. Prove the SPP transaction, whose real input the maker owns.
    /// 2. Prove the make circuit and build the instruction.
    pub fn prove(
        self,
        indexer: &SprIndexer,
        tree: Address,
        maker: &ShieldedKeypair,
    ) -> Result<Make> {
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp_proof = prove_transact(indexer, self.spp_proof_inputs, maker)?;
        // 2. Prove the make circuit and build the instruction.
        let make_proof = proof::prove(CircuitId::Make, &self.proof_inputs)?;
        Ok(Make {
            viewing_pubkey: self.viewing_pubkey,
            payer,
            tree,
            make_proof,
            spp_proof,
        })
    }
}

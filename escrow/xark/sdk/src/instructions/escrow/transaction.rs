use anyhow::{anyhow, Result};
use solana_address::Address;
use spr_client::SprIndexer;
use spr_keypair::ShieldedKeypair;
use spr_program::{ProgramProofInputs, TransactionOptions};
use spr_transaction::{instructions::transact::SppProofInputs, SppProofOutputUtxo, WalletUtxo};
use timelock_escrow_program::circuits::{
    EscrowClient, EscrowPrivateInputsClient, EscrowPublicInputsClient,
};

use super::Escrow;
use crate::{
    err, escrow_authority,
    proof::{self, CircuitId},
    shared::{owner_pk_field, prove_transact},
};

/// Output slot of the change in the escrow transaction.
const CHANGE_SLOT: usize = 0;
/// Output slot of the escrow UTXO in the escrow transaction.
pub const ESCROW_SLOT: usize = 1;

pub struct EscrowTransactionParams {
    /// Unix seconds after which the creator can withdraw.
    pub unlock_timestamp: u64,
    /// The amount locked in the escrow UTXO; the rest of `source` is change.
    pub amount: u64,
    /// The spent source UTXO, owned by the creator. Its tree is the output
    /// tree.
    pub source: WalletUtxo,
    pub payer: Address,
}

pub struct EscrowTransaction {
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ProgramProofInputs,
    /// The escrow UTXO output, with its derived blinding and its terms as
    /// data. [`crate::state::escrow_utxo`] turns it into the withdraw input.
    pub escrow_output: SppProofOutputUtxo,
    /// The change output's blinding.
    pub change_blinding: [u8; 32],
}

impl EscrowTransactionParams {
    /// 1. Build the program transaction; the circuit sets the terms.
    /// 2. Read the escrow UTXO and the change it created.
    /// 3. Encrypt the outputs.
    pub fn build(self, creator: &ShieldedKeypair) -> Result<EscrowTransaction> {
        let sender = creator.shielded_address().map_err(err)?;

        // 1. Build the program transaction; the circuit sets the terms.
        let transaction = EscrowClient {
            private: EscrowPrivateInputsClient {
                tx_context: TransactionOptions {
                    blinding_seed: None,
                    output_tree_id: Some(self.source.tree_id),
                },
                source: self.source,
                amount: self.amount,
                unlock: self.unlock_timestamp,
                escrow_address: escrow_authority()
                    .address(sender.viewing_pubkey)
                    .map_err(err)?,
            },
            public: EscrowPublicInputsClient {
                owner_pk_field: owner_pk_field(creator)?,
            },
        }
        .prepare(&sender, self.payer)
        .map_err(err)?;
        let finalized = transaction.finalized;

        // 2. Read the escrow UTXO and the change it created.
        let outputs = finalized.output_utxos();
        let change_blinding = outputs
            .get(CHANGE_SLOT)
            .ok_or_else(|| anyhow!("the escrow transaction has no change output"))?
            .blinding;
        let escrow_output = outputs
            .get(ESCROW_SLOT)
            .ok_or_else(|| anyhow!("the escrow transaction has no escrow output"))?
            .clone();

        // 3. Encrypt the outputs.
        let key = creator
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(EscrowTransaction {
            spp_proof_inputs,
            proof_inputs: transaction.proof_inputs,
            escrow_output,
            change_blinding,
        })
    }
}

impl EscrowTransaction {
    /// 1. Prove the SPP transaction, with the creator's key for its source.
    /// 2. Prove the escrow circuit and build the instruction.
    pub fn prove(
        self,
        indexer: &SprIndexer,
        tree: Address,
        creator: &ShieldedKeypair,
    ) -> Result<Escrow> {
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction, with the creator's key for its source.
        let spp_proof = prove_transact(indexer, self.spp_proof_inputs, creator)?;
        // 2. Prove the escrow circuit and build the instruction.
        let escrow_proof = proof::prove(CircuitId::Escrow, &self.proof_inputs)?;
        Ok(Escrow {
            payer,
            tree,
            escrow_proof,
            spp_proof,
        })
    }
}

use anyhow::{anyhow, ensure, Result};
use solana_address::Address;
use spr_client::SprIndexer;
use spr_keypair::ShieldedKeypair;
use spr_program::{ProgramProofInputs, TransactionOptions};
use spr_transaction::{instructions::transact::SppProofInputs, WalletUtxo};
use timelock_escrow_program::circuits::{
    EscrowClient, EscrowPrivateInputsClient, EscrowPublicInputsClient,
};

use super::Escrow;
use crate::{
    err, escrow_authority,
    proof::{self, CircuitId},
    shared::prove_pda_transact,
    state::{EscrowTerms, EscrowUtxo},
};

/// Output slot of the change in the escrow transaction.
pub const CHANGE_SLOT: usize = 0;
/// Output slot of the escrow UTXO in the escrow transaction.
pub const ESCROW_SLOT: usize = 1;

pub struct EscrowTransactionParams {
    pub terms: EscrowTerms,
    /// The amount locked in the escrow UTXO; the rest of `source` is change.
    pub amount: u64,
    /// The spent source UTXO. The transaction is signed by the program alone,
    /// so it must be owned by the escrow-authority PDA, as a proofless
    /// deposit to [`crate::escrow_authority`]'s address is.
    pub source: WalletUtxo,
    pub payer: Address,
    /// The output tree, which also holds `source`.
    pub tree_id: u16,
}

pub struct EscrowTransaction {
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ProgramProofInputs,
    /// The escrow UTXO the transaction creates, with its derived blinding.
    pub escrow_utxo: EscrowUtxo,
    /// The change output's blinding.
    pub change_blinding: [u8; 32],
}

impl EscrowTransactionParams {
    /// 1. Check the terms name the creator.
    /// 2. Build the program transaction.
    /// 3. Read the escrow UTXO and the change it created.
    /// 4. Encrypt the outputs.
    pub fn build(self, creator: &ShieldedKeypair) -> Result<EscrowTransaction> {
        // 1. Check the terms name the creator.
        let sender = creator.shielded_address().map_err(err)?;
        ensure!(
            self.terms.creator == sender,
            "the escrow terms name another creator"
        );
        ensure!(
            self.source.tree_id == self.tree_id,
            "the source is in tree {}, not in the output tree {}",
            self.source.tree_id,
            self.tree_id
        );
        let asset = self.source.utxo.asset;

        // 2. Build the program transaction.
        let transaction = EscrowClient {
            private: EscrowPrivateInputsClient {
                tx_context: TransactionOptions {
                    blinding_seed: None,
                    output_tree_id: Some(self.tree_id),
                },
                terms: self.terms.circuit_terms()?,
                amount: self.amount,
                source: self.source,
                creator: sender,
                escrow_address: escrow_authority()
                    .address(sender.viewing_pubkey)
                    .map_err(err)?,
            },
            public: EscrowPublicInputsClient {},
        }
        .prepare(&sender, self.payer)
        .map_err(err)?;
        let finalized = transaction.finalized;

        // 3. Read the escrow UTXO and the change it created.
        let outputs = finalized.output_utxos();
        let change = outputs
            .get(CHANGE_SLOT)
            .ok_or_else(|| anyhow!("the escrow transaction has no change output"))?;
        let escrow_output = outputs
            .get(ESCROW_SLOT)
            .ok_or_else(|| anyhow!("the escrow transaction has no escrow output"))?;
        let escrow_utxo = EscrowUtxo {
            terms: self.terms,
            blinding: escrow_output.blinding,
            asset,
            amount: self.amount,
        };
        ensure!(
            escrow_utxo.hash(self.tree_id)? == escrow_output.hash(self.tree_id).map_err(err)?,
            "the escrow output does not commit the escrow terms"
        );
        let change_blinding = change.blinding;

        // 4. Encrypt the outputs.
        let key = creator
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(EscrowTransaction {
            spp_proof_inputs,
            proof_inputs: transaction.proof_inputs,
            escrow_utxo,
            change_blinding,
        })
    }
}

impl EscrowTransaction {
    /// 1. Prove the SPP transaction.
    /// 2. Prove the escrow circuit and build the instruction.
    pub fn prove(self, indexer: &SprIndexer, tree: Address) -> Result<Escrow> {
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction.
        let spp_proof = prove_pda_transact(indexer, self.spp_proof_inputs)?;
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

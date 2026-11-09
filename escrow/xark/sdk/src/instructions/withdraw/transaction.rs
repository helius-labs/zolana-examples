use anyhow::{anyhow, ensure, Result};
use solana_address::Address;
use spr_client::SprIndexer;
use spr_keypair::ShieldedKeypair;
use spr_program::{InputRecords, ProgramOwner, ProgramProofInputs, TransactionOptions};
use spr_transaction::instructions::transact::SppProofInputs;
use timelock_escrow_program::circuits::{
    WithdrawClient, WithdrawPrivateInputsClient, WithdrawPublicInputsClient,
};

use super::Withdraw;
use crate::{
    err,
    proof::{self, CircuitId},
    shared::{owner_pk_field, prove_transact},
    state::EscrowUtxo,
};

/// Output slot of the withdrawn source output in the withdraw transaction.
pub const SOURCE_OUTPUT_SLOT: usize = 0;

pub struct WithdrawTransactionParams {
    /// The escrow UTXO with its decoded terms, from
    /// [`crate::state::open_escrows`] or [`crate::state::escrow_utxo`]. Its
    /// tree is also the output tree.
    pub escrow: EscrowUtxo,
    pub payer: Address,
}

pub struct WithdrawTransaction {
    creator: Address,
    unlock_timestamp: u64,
    pub spp_proof_inputs: SppProofInputs,
    pub proof_inputs: ProgramProofInputs,
    /// The source output's blinding.
    pub source_output_blinding: [u8; 32],
}

impl WithdrawTransactionParams {
    /// 1. Check the creator withdraws.
    /// 2. Build the program transaction.
    /// 3. Encrypt the source output to the creator.
    pub fn build(self, creator: &ShieldedKeypair) -> Result<WithdrawTransaction> {
        // 1. Check the creator withdraws.
        let sender = creator.shielded_address().map_err(err)?;
        let terms = &self.escrow.data;
        let creator_owner = *InputRecords::new().address(&sender).map_err(err)?.owner();
        ensure!(
            terms.creator == creator_owner,
            "only the escrow creator can withdraw it"
        );
        let unlock_timestamp = terms.unlock;
        let tree_id = self.escrow.utxo.tree_id;

        // 2. Build the program transaction.
        let transaction = WithdrawClient {
            private: WithdrawPrivateInputsClient {
                tx_context: TransactionOptions {
                    blinding_seed: None,
                    output_tree_id: Some(tree_id),
                },
                escrow: self.escrow.utxo,
                terms: self.escrow.data.into(),
            },
            public: WithdrawPublicInputsClient {
                unlock: unlock_timestamp,
                owner_pk_field: owner_pk_field(creator)?,
            },
        }
        .prepare(&sender, self.payer)
        .map_err(err)?;
        let finalized = transaction.finalized;
        let source_output_blinding = finalized
            .output_utxos()
            .get(SOURCE_OUTPUT_SLOT)
            .ok_or_else(|| anyhow!("the withdraw transaction has no source output"))?
            .blinding;

        // 3. Encrypt the source output to the creator.
        let key = creator
            .get_transaction_viewing_key(&finalized.first_nullifier().map_err(err)?)
            .map_err(err)?;
        let spp_proof_inputs = finalized.encrypt_with_viewing_key(&key).map_err(err)?;
        Ok(WithdrawTransaction {
            creator: sender.solana_address().map_err(err)?,
            unlock_timestamp,
            spp_proof_inputs,
            proof_inputs: transaction.proof_inputs,
            source_output_blinding,
        })
    }
}

impl WithdrawTransaction {
    /// 1. Prove the SPP transaction, with the PDA's zero nullifier key for the
    ///    escrow UTXO.
    /// 2. Prove the withdraw circuit and build the instruction.
    pub fn prove(self, indexer: &SprIndexer, tree: Address) -> Result<Withdraw> {
        let payer = self.spp_proof_inputs.payer;
        // 1. Prove the SPP transaction, with the PDA's zero nullifier key for
        // the escrow UTXO.
        let spp_proof = prove_transact(
            indexer,
            self.spp_proof_inputs,
            &ProgramOwner::nullifier_key(),
        )?;
        // 2. Prove the withdraw circuit and build the instruction.
        let withdraw_proof = proof::prove(CircuitId::Withdraw, &self.proof_inputs)?;
        Ok(Withdraw {
            creator: self.creator,
            payer,
            tree,
            withdraw_proof,
            unlock_timestamp: self.unlock_timestamp,
            spp_proof,
        })
    }
}

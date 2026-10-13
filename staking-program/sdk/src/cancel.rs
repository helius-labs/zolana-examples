use anyhow::{ensure, Result};
use p256::SecretKey;
use private_staking_program::{
    instructions::{
        cancel::{proof_context, CancelProofContext},
        proof::LogicProof,
    },
    state::Config,
};
use private_staking_prover::{CircuitId, ProofInputMap, PROVER};
use solana_address::Address;
use zolana_hasher::primitives::right_align;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    encryption::{self, Context},
    intent::{scalar, write_encryption, write_intent, write_note},
    note::Note,
};

pub struct CancelProofParams<'a> {
    pub config_key: Address,
    pub config: &'a Config,
    pub epoch: Address,
    pub signer: Address,
    pub spp: &'a TransactIxData,
    pub escrow: Note,
    pub refund: Note,
    pub private_tx_blinding: [u8; 32],
    pub ephemeral: SecretKey,
}

pub struct CancelProofInputs {
    pub context: CancelProofContext,
    pub escrow: Note,
    pub refund: Note,
    pub private_tx_blinding: [u8; 32],
    pub ephemeral: SecretKey,
    pub crank_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
}

impl CancelProofInputs {
    pub fn new(params: CancelProofParams<'_>) -> Result<Self> {
        let CancelProofParams {
            config_key,
            config,
            epoch,
            signer,
            spp,
            escrow,
            refund,
            private_tx_blinding,
            ephemeral,
        } = params;
        let intent = escrow
            .intent
            .ok_or_else(|| anyhow::anyhow!("missing intent data"))?;
        ensure!(intent.epoch == epoch, "wrong intent epoch");
        ensure!(
            refund.intent.is_none(),
            "refund must not create another escrow"
        );
        let context = proof_context(
            &config_key,
            config,
            &epoch,
            &signer,
            escrow.tree_id,
            refund.tree_id,
            spp,
        )?;
        let output = spp
            .outputs
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing refund output"))?;
        let encryption_context = Context {
            tx_viewing_pk: spp.tx_viewing_pk,
            salt: spp.salt,
            output_index: 0,
            utxo_hash: output.utxo_hash,
        };
        let ciphertext = encryption::encrypt(
            &refund,
            &ephemeral,
            &intent.recipient.viewing_pubkey,
            &config.crank.viewing_pubkey,
            &encryption_context,
        )?;
        ensure!(
            output.data.as_deref() == Some(&ciphertext),
            "refund ciphertext mismatch"
        );
        Ok(Self {
            context,
            escrow,
            refund,
            private_tx_blinding,
            ephemeral,
            crank_viewing_pk: config.crank.viewing_pubkey,
            salt: spp.salt,
        })
    }

    pub fn witness(&self) -> Result<ProofInputMap> {
        let mut map = ProofInputMap::new();
        let c = self.context;
        for (name, value) in [
            ("PublicInputHash", c.hash()?),
            ("PrivateTxHash", c.private_tx_hash),
            ("PrivateTxBlinding", self.private_tx_blinding),
            ("NullifierHash", c.nullifier_hash),
            ("EscrowOwnerIdentity", c.escrow_owner_identity),
            ("SignerIdentity", c.signer_identity),
            ("EpochHash", c.epoch_hash),
            ("PoolMintHash", c.pool_mint_hash),
            ("CrankViewingHash", c.crank_viewing_hash),
            ("InputTreeID", right_align(&c.input_tree_id.to_be_bytes())),
            ("OutputTreeID", right_align(&c.output_tree_id.to_be_bytes())),
            ("CiphertextContextHash", c.ciphertext_context_hash),
        ] {
            scalar(&mut map, name, &value);
        }
        write_note(&mut map, "Escrow", &self.escrow);
        write_note(&mut map, "Refund", &self.refund);
        let intent = self
            .escrow
            .intent
            .ok_or_else(|| anyhow::anyhow!("missing intent data"))?;
        write_intent(&mut map, &intent);
        write_encryption(
            &mut map,
            &self.ephemeral,
            &intent.recipient.viewing_pubkey,
            &self.crank_viewing_pk,
            &self.salt,
        )?;
        Ok(map)
    }

    pub fn prove(&self) -> Result<LogicProof> {
        let proof = PROVER
            .prove(CircuitId::Cancel, &self.witness()?)?
            .compress()?;
        let (commitment, commitment_pok) = proof
            .commitment
            .ok_or_else(|| anyhow::anyhow!("missing BSB22 commitment"))?;
        Ok(LogicProof {
            a: proof.proof_a,
            b: proof.proof_b,
            c: proof.proof_c,
            commitment,
            commitment_pok,
        })
    }
}

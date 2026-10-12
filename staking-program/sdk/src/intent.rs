use anyhow::{ensure, Result};
use p256::{elliptic_curve::sec1::ToEncodedPoint, PublicKey, SecretKey};
use private_staking_program::{
    instructions::{
        intent::{proof_context, IntentProofContext},
        proof::LogicProof,
    },
    state::Config,
};
use private_staking_prover::{CircuitId, ProofInputMap, PROVER};
use solana_address::Address;
use zolana_client::ProofInputUtxo;
use zolana_gnark_ffi_prover::{decimal, utxo_proof_inputs};
use zolana_hasher::primitives::right_align;
use zolana_interface::instruction::instruction_data::transact::TransactIxData;

use crate::{
    encryption::{self, Context},
    note::Note,
};

#[derive(Clone, Debug, Default)]
pub struct FundingInput {
    pub utxo: ProofInputUtxo,
    pub owner_identity: [u8; 32],
    pub nullifier_pk: [u8; 32],
}

pub struct IntentProofInputs {
    pub context: IntentProofContext,
    pub funding: [FundingInput; 2],
    pub escrow: Note,
    pub change: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
    pub ephemeral: SecretKey,
    pub crank_viewing_pk: [u8; 33],
    pub salt: [u8; 16],
}

pub struct IntentProofParams<'a> {
    pub config_key: Address,
    pub config: &'a Config,
    pub epoch: Address,
    pub spp: &'a TransactIxData,
    pub escrow: Note,
    pub funding: [FundingInput; 2],
    pub change: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
    pub ephemeral: SecretKey,
}

fn scalar(map: &mut ProofInputMap, name: &str, value: &[u8; 32]) {
    map.insert(name.into(), vec![decimal(value)]);
}

fn bytes(map: &mut ProofInputMap, name: &str, value: &[u8]) {
    map.insert(name.into(), value.iter().map(u8::to_string).collect());
}

impl IntentProofInputs {
    pub fn new(params: IntentProofParams<'_>) -> Result<Self> {
        let IntentProofParams {
            config_key,
            config,
            epoch,
            spp,
            escrow,
            funding,
            change,
            private_tx_blinding,
            ephemeral,
        } = params;
        let intent = escrow
            .intent
            .ok_or_else(|| anyhow::anyhow!("missing intent data"))?;
        ensure!(intent.epoch == epoch, "wrong intent epoch");
        let context = proof_context(&config_key, config, &epoch, escrow.tree_id, spp)?;
        let cipher_context = Context {
            tx_viewing_pk: spp.tx_viewing_pk,
            salt: spp.salt,
            output_index: 0,
            utxo_hash: context.escrow_hash,
        };
        let ciphertext = encryption::encrypt(
            &escrow,
            &ephemeral,
            &intent.recipient.viewing_pubkey,
            &config.crank.viewing_pubkey,
            &cipher_context,
        )?;
        ensure!(
            spp.outputs
                .first()
                .ok_or_else(|| anyhow::anyhow!("missing escrow output"))?
                .data
                .as_deref()
                == Some(&ciphertext),
            "intent ciphertext mismatch"
        );
        Ok(Self {
            context,
            funding,
            escrow,
            change,
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
            ("EscrowHash", c.escrow_hash),
            ("EscrowOwnerIdentity", c.escrow_owner_identity),
            ("EpochHash", c.epoch_hash),
            ("PoolMintHash", c.pool_mint_hash),
            ("CrankViewingHash", c.crank_viewing_hash),
            ("TreeID", right_align(&c.tree_id.to_be_bytes())),
            ("InputCount", right_align(&[c.input_count])),
            ("OutputCount", right_align(&[c.output_count])),
            ("CiphertextContextHash", c.ciphertext_context_hash),
        ] {
            scalar(&mut map, name, &value);
        }
        for (i, input) in self.funding.iter().enumerate() {
            map.extend(utxo_proof_inputs(&input.utxo, &format!("Input{i}")));
        }
        map.insert(
            "InputOwnerIdentity".into(),
            self.funding
                .iter()
                .map(|input| decimal(&input.owner_identity))
                .collect(),
        );
        map.insert(
            "InputNullifierPK".into(),
            self.funding
                .iter()
                .map(|input| decimal(&input.nullifier_pk))
                .collect(),
        );
        map.extend(utxo_proof_inputs(&self.change, "Change"));
        let n = &self.escrow;
        for (name, value) in [
            ("Escrow_TreeID", right_align(&n.tree_id.to_be_bytes())),
            ("Escrow_AssetID", right_align(&n.asset_id.to_be_bytes())),
            ("Escrow_Amount", right_align(&n.amount.to_be_bytes())),
            ("Escrow_Blinding", n.blinding),
            ("Escrow_NullifierPK", n.nullifier_pubkey),
        ] {
            scalar(&mut map, name, &value);
        }
        bytes(&mut map, "Escrow_Asset", n.asset.as_ref());
        bytes(&mut map, "Escrow_SigningPK", &n.signing_pubkey);
        let intent = n
            .intent
            .ok_or_else(|| anyhow::anyhow!("missing intent data"))?;
        bytes(&mut map, "Intent_Epoch", intent.epoch.as_ref());
        bytes(
            &mut map,
            "Intent_SigningPK",
            &intent.recipient.signing_pubkey,
        );
        scalar(
            &mut map,
            "Intent_NullifierPK",
            &intent.recipient.nullifier_pubkey,
        );
        bytes(
            &mut map,
            "Intent_ViewingPK",
            &intent.recipient.viewing_pubkey,
        );
        bytes(
            &mut map,
            "Encryption_EphemeralSecret",
            &self.ephemeral.to_bytes(),
        );
        let user = PublicKey::from_sec1_bytes(&intent.recipient.viewing_pubkey)?;
        let crank = PublicKey::from_sec1_bytes(&self.crank_viewing_pk)?;
        bytes(
            &mut map,
            "Encryption_UserPoint",
            user.to_encoded_point(false).as_bytes(),
        );
        bytes(
            &mut map,
            "Encryption_CrankPoint",
            crank.to_encoded_point(false).as_bytes(),
        );
        bytes(
            &mut map,
            "Encryption_TxViewingPK",
            self.ephemeral
                .public_key()
                .to_encoded_point(true)
                .as_bytes(),
        );
        bytes(&mut map, "Encryption_Salt", &self.salt);
        scalar(&mut map, "Encryption_OutputIndex", &[0; 32]);
        Ok(map)
    }

    pub fn prove(&self) -> Result<LogicProof> {
        let proof = PROVER
            .prove(CircuitId::Intent, &self.witness()?)?
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

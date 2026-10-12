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

pub(crate) fn scalar(map: &mut ProofInputMap, name: &str, value: &[u8; 32]) {
    map.insert(name.into(), vec![decimal(value)]);
}

pub(crate) fn bytes(map: &mut ProofInputMap, name: &str, value: &[u8]) {
    map.insert(name.into(), value.iter().map(u8::to_string).collect());
}

pub(crate) fn write_note(map: &mut ProofInputMap, prefix: &str, note: &Note) {
    for (name, value) in [
        ("TreeID", right_align(&note.tree_id.to_be_bytes())),
        ("AssetID", right_align(&note.asset_id.to_be_bytes())),
        ("Amount", right_align(&note.amount.to_be_bytes())),
        ("Blinding", note.blinding),
        ("NullifierPK", note.nullifier_pubkey),
    ] {
        scalar(map, &format!("{prefix}_{name}"), &value);
    }
    bytes(map, &format!("{prefix}_Asset"), note.asset.as_ref());
    bytes(map, &format!("{prefix}_SigningPK"), &note.signing_pubkey);
}

pub(crate) fn write_intent(
    map: &mut ProofInputMap,
    data: &private_staking_program::state::IntentUtxoData,
) {
    bytes(map, "Intent_Epoch", data.epoch.as_ref());
    bytes(map, "Intent_SigningPK", &data.recipient.signing_pubkey);
    scalar(map, "Intent_NullifierPK", &data.recipient.nullifier_pubkey);
    bytes(map, "Intent_ViewingPK", &data.recipient.viewing_pubkey);
}

pub(crate) fn write_encryption(
    map: &mut ProofInputMap,
    ephemeral: &SecretKey,
    user: &[u8; 33],
    crank: &[u8; 33],
    salt: &[u8; 16],
) -> Result<()> {
    bytes(map, "Encryption_EphemeralSecret", &ephemeral.to_bytes());
    let user = PublicKey::from_sec1_bytes(user)?;
    let crank = PublicKey::from_sec1_bytes(crank)?;
    bytes(
        map,
        "Encryption_UserPoint",
        user.to_encoded_point(false).as_bytes(),
    );
    bytes(
        map,
        "Encryption_CrankPoint",
        crank.to_encoded_point(false).as_bytes(),
    );
    bytes(
        map,
        "Encryption_TxViewingPK",
        ephemeral.public_key().to_encoded_point(true).as_bytes(),
    );
    bytes(map, "Encryption_Salt", salt);
    scalar(map, "Encryption_OutputIndex", &[0; 32]);
    Ok(())
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
        ensure!(
            matches!(intent.recipient.signing_pubkey.first(), Some(1 | 2)),
            "staking recipients need a Solana signing key"
        );
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
            (
                "MinDepositLamports",
                right_align(&c.min_deposit_lamports.to_be_bytes()),
            ),
            (
                "MinWithdrawalPoolTokens",
                right_align(&c.min_withdrawal_pool_tokens.to_be_bytes()),
            ),
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
        write_note(&mut map, "Escrow", &self.escrow);
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

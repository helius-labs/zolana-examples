use anyhow::{bail, Result};
use spr_keypair::X25519Pubkey;
use spr_transaction::{
    instructions::transact::{PrivateTxHash, SppProofOutputUtxo},
    utxo::ProofInputUtxo,
};
use swap_program::circuits::{Cancel, CancelPrivateInputs, CancelPublicInputs};

use crate::{
    err,
    shared::{check_output_utxo, circuit_utxo, field},
    state::OrderUtxo,
};

pub struct CancelProofInputParams {
    pub order_utxo: OrderUtxo,
    pub taker_viewing_pubkey: X25519Pubkey,
    pub source_output: SppProofOutputUtxo,
    /// `SppProofInputs::private_tx_blinding()`, the last `private_tx_hash`
    /// preimage element.
    pub private_tx_blinding: [u8; 32],
    /// Raw id of the tree the order UTXO is spent from.
    pub input_tree_id: u16,
    /// Raw id of the tree the source output is appended to.
    pub output_tree_id: u16,
}

impl CancelProofInputParams {
    pub fn to_proof_inputs(&self) -> Result<Cancel> {
        let terms = &self.order_utxo.terms;
        let maker = check_output_utxo(
            "source_output",
            &self.source_output,
            &self.order_utxo.source_mint.asset,
            self.order_utxo.source_amount,
        )?;
        if maker != terms.destination {
            bail!("source output owner does not match the order destination");
        }
        let order = terms.circuit_terms()?;
        let maker_owner_pk_field = maker.signing_pubkey.owner_proof_input_hash().map_err(err)?;
        let order_utxo = ProofInputUtxo::try_from((
            &self
                .order_utxo
                .output_utxo(self.order_utxo.terms.destination.viewing_pubkey)?,
            self.input_tree_id,
        ))
        .map_err(err)?;
        let source_output =
            ProofInputUtxo::try_from((&self.source_output, self.output_tree_id)).map_err(err)?;
        let private_tx_hash = PrivateTxHash::new(
            &[order_utxo.hash().map_err(err)?],
            &[source_output.hash().map_err(err)?],
            &self.private_tx_blinding,
        )
        .hash()
        .map_err(err)?;
        Ok(Cancel {
            private: CancelPrivateInputs {
                private_tx_hash: field(&private_tx_hash)?,
                order,
                order_utxo: circuit_utxo(&order_utxo)?,
                source_output: circuit_utxo(&source_output)?,
                maker_nullifier_pk: field(&maker.nullifier_pubkey)?,
                private_tx_blinding: field(&self.private_tx_blinding)?,
            },
            public: CancelPublicInputs {
                expiry: terms.expiry,
                maker_owner_pk_field: field(&maker_owner_pk_field)?,
            },
        })
    }
}

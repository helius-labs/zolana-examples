use anyhow::{bail, Result};
use spr_client::ProofInputUtxo;
use spr_transaction::instructions::transact::{PrivateTxHash, SppProofInputs, SppProofOutputUtxo};
use timelock_escrow_program::instructions::escrow::EscrowPublicInput;
use timelock_escrow_prover::{EscrowProofInputs, EscrowTermsProofInput};

use crate::{err, state::EscrowUtxo};

/// The parts of the SPP transact this escrow CPIs into that the escrow proof has
/// to reproduce byte-for-byte, or the two proofs bind different
/// `private_tx_hash` values and the instruction can never land.
pub struct SppTxHashes {
    /// The spent source, the transaction's first input, hashed under the tree
    /// it is spent from. The escrow circuit requires the creator to own it.
    pub source: ProofInputUtxo,
    /// `SppProofInputs::private_tx_blinding()`, the last `private_tx_hash`
    /// preimage element.
    pub private_tx_blinding: [u8; 32],
    /// Raw id of the tree the escrow and change outputs are appended to; it is
    /// the second element of every output's commitment.
    pub output_tree_id: u16,
}

impl SppTxHashes {
    pub fn new(spp_proof_inputs: &SppProofInputs) -> Result<Self> {
        let source_input = spp_proof_inputs
            .input_utxos
            .first()
            .ok_or_else(|| err("missing source input"))?;
        Ok(Self {
            source: ProofInputUtxo::try_from(source_input).map_err(err)?,
            private_tx_blinding: spp_proof_inputs.private_tx_blinding().map_err(err)?,
            output_tree_id: spp_proof_inputs.output_tree_id,
        })
    }
}

pub struct EscrowProofInputParams {
    pub escrow_utxo: EscrowUtxo,
    pub change: SppProofOutputUtxo,
    pub spp_tx_hashes: SppTxHashes,
}

impl EscrowProofInputParams {
    pub fn to_proof_inputs(&self) -> Result<EscrowProofInputs> {
        let terms = &self.escrow_utxo.terms;
        if self.change.owner_address != Some(terms.creator) {
            bail!("change owner does not match escrow creator");
        }
        if self.change.asset != self.escrow_utxo.asset {
            bail!("change asset does not match escrow asset");
        }
        if self.change.data_hash.is_some()
            || self.change.ring_data_hash.is_some()
            || self.change.ring_program_id.is_some()
        {
            bail!("change output must not carry data or ring commitments");
        }
        let terms_input = EscrowTermsProofInput::try_from(terms)?;
        // Both are created by this transaction, so both commit under the output
        // tree's id.
        let output_tree_id = self.spp_tx_hashes.output_tree_id;
        let escrow_utxo =
            ProofInputUtxo::try_from((&self.escrow_utxo.output_utxo()?, output_tree_id))
                .map_err(err)?;
        let escrow_owner_hash = self.escrow_utxo.owner_hash()?;
        if escrow_utxo.owner_hash != escrow_owner_hash {
            bail!("escrow utxo owner is not the creator's escrow-authority PDA");
        }
        // The creator funds the escrow from its own balance and signs the
        // spend. A utxo of the creator's escrow-authority PDA, which the
        // program signs for, must not fund it: the creator could re-lock its
        // own escrow and take the rest as change before unlock.
        let source = &self.spp_tx_hashes.source;
        if source.owner_hash != terms_input.owner_hash {
            bail!("source is not owned by the escrow creator");
        }
        if source.asset != escrow_utxo.asset {
            bail!("source asset does not match escrow asset");
        }
        if source.data_hash != [0u8; 32]
            || source.ring_data_hash != [0u8; 32]
            || source.ring_program_id != [0u8; 32]
        {
            bail!("source must not carry data or ring commitments");
        }
        let change = ProofInputUtxo::try_from((&self.change, output_tree_id)).map_err(err)?;
        let private_tx_hash = PrivateTxHash::new(
            &[source.hash().map_err(err)?, [0u8; 32]],
            &[
                change.hash().map_err(err)?,
                escrow_utxo.hash().map_err(err)?,
            ],
            &self.spp_tx_hashes.private_tx_blinding,
        )
        .hash()
        .map_err(err)?;
        let owner_pk_field = terms
            .creator
            .signing_pubkey
            .owner_proof_input_hash()
            .map_err(err)?;
        let public_input_hash = EscrowPublicInput {
            private_tx_hash: &private_tx_hash,
            escrow_owner_hash: &escrow_owner_hash,
            owner_pk_field: &owner_pk_field,
        }
        .hash()
        .map_err(err)?;
        Ok(EscrowProofInputs {
            public_input_hash,
            private_tx_hash,
            escrow_owner_hash,
            owner_pk_field,
            nullifier_pk: terms.creator.nullifier_pubkey,
            terms: terms_input,
            source: source.clone(),
            escrow_utxo,
            change,
            private_tx_blinding: self.spp_tx_hashes.private_tx_blinding,
        })
    }
}

#[cfg(test)]
mod tests {
    use spr_keypair::ShieldedKeypair;
    use spr_transaction::{Mint, SOL_MINT};

    use super::*;
    use crate::state::EscrowTerms;

    const TREE_ID: u16 = 1;

    fn field(byte: u8) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[31] = byte;
        out
    }

    fn escrow_utxo() -> EscrowUtxo {
        EscrowUtxo {
            terms: EscrowTerms {
                creator: ShieldedKeypair::new_ed25519()
                    .unwrap()
                    .shielded_address()
                    .unwrap(),
                unlock_timestamp: 42,
            },
            blinding: field(7),
            asset: Mint::SOL,
            amount: 250,
        }
    }

    fn params(escrow_utxo: EscrowUtxo, source: ProofInputUtxo) -> EscrowProofInputParams {
        let change = SppProofOutputUtxo::new(Mint::SOL, 750, escrow_utxo.terms.creator).unwrap();
        EscrowProofInputParams {
            escrow_utxo,
            change,
            spp_tx_hashes: SppTxHashes {
                source,
                private_tx_blinding: field(21),
                output_tree_id: TREE_ID,
            },
        }
    }

    fn token_utxo(owner_hash: [u8; 32]) -> ProofInputUtxo {
        ProofInputUtxo::new(owner_hash, &SOL_MINT, 1_000, &field(5), TREE_ID).unwrap()
    }

    #[test]
    fn creator_owned_source_builds_the_witness_source() {
        let escrow_utxo = escrow_utxo();
        let source = token_utxo(escrow_utxo.terms.creator.owner_hash().unwrap());
        let inputs = params(escrow_utxo, source.clone())
            .to_proof_inputs()
            .unwrap();
        assert_eq!(inputs.source, source);
    }

    /// A source of the creator's escrow-authority PDA, such as the creator's
    /// own escrow utxo, would let the creator re-lock its escrow before
    /// unlock.
    #[test]
    fn rejects_source_not_owned_by_creator() {
        let escrow_utxo = escrow_utxo();
        let own_escrow =
            ProofInputUtxo::try_from((&escrow_utxo.output_utxo().unwrap(), TREE_ID)).unwrap();
        let pda_token = token_utxo(escrow_utxo.owner_hash().unwrap());
        for source in [own_escrow, pda_token] {
            let error = params(escrow_utxo.clone(), source)
                .to_proof_inputs()
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "source is not owned by the escrow creator"
            );
        }
    }
}

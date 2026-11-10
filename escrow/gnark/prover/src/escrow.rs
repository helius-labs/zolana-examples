use std::collections::HashMap;

use spr_client::ProofInputUtxo;
use spr_gnark_ffi_prover::{decimal, utxo_proof_inputs, ProofInputMap};

use crate::{CircuitId, EscrowTermsProofInput, TimelockProof, PROVER};

#[derive(Debug, Clone)]
pub struct EscrowProofInputs {
    pub public_input_hash: [u8; 32],
    pub private_tx_hash: [u8; 32],
    pub escrow_owner_hash: [u8; 32],
    pub owner_pk_field: [u8; 32],
    pub nullifier_pk: [u8; 32],
    pub terms: EscrowTermsProofInput,
    /// The spent source, owned by the creator.
    pub source: ProofInputUtxo,
    pub escrow_utxo: ProofInputUtxo,
    pub change: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
}

impl EscrowProofInputs {
    fn witness(&self) -> ProofInputMap {
        let scalars: [(&str, [u8; 32]); 6] = [
            ("Public_PublicInputHash", self.public_input_hash),
            ("Public_PrivateTxHash", self.private_tx_hash),
            ("Public_EscrowOwnerHash", self.escrow_owner_hash),
            ("OwnerPkField", self.owner_pk_field),
            ("NullifierPk", self.nullifier_pk),
            ("PrivateTxBlinding", self.private_tx_blinding),
        ];
        let mut map = HashMap::new();
        for (key, value) in scalars.iter() {
            map.insert(key.to_string(), vec![decimal(value)]);
        }
        for (key, value) in self
            .terms
            .witness_entries("Terms")
            .into_iter()
            .chain(utxo_proof_inputs(&self.source, "Source"))
            .chain(utxo_proof_inputs(&self.escrow_utxo, "EscrowUtxo"))
            .chain(utxo_proof_inputs(&self.change, "Change"))
        {
            map.insert(key, value);
        }
        map
    }

    pub fn prove(&self) -> spr_gnark_ffi_prover::Result<TimelockProof> {
        Ok(PROVER
            .prove(CircuitId::Escrow, &self.witness())?
            .compress()?
            .into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use spr_gnark_ffi_prover::utxo_proof_input_keys;

    use super::*;
    use crate::escrow_terms::expected_escrow_terms_witness_keys;

    fn sample() -> EscrowProofInputs {
        EscrowProofInputs {
            public_input_hash: [8; 32],
            private_tx_hash: [1; 32],
            escrow_owner_hash: [9; 32],
            owner_pk_field: [10; 32],
            nullifier_pk: [11; 32],
            terms: EscrowTermsProofInput {
                owner_hash: [2; 32],
                unlock: 42,
            },
            source: ProofInputUtxo::default(),
            escrow_utxo: ProofInputUtxo::default(),
            change: ProofInputUtxo::default(),
            private_tx_blinding: [5; 32],
        }
    }

    #[test]
    fn witness_key_set_matches_circuit_fields() {
        let witness = sample().witness();
        let keys: HashSet<String> = witness.keys().cloned().collect();

        let mut expected: Vec<String> = vec![
            "Public_PublicInputHash".to_string(),
            "Public_PrivateTxHash".to_string(),
            "Public_EscrowOwnerHash".to_string(),
            "OwnerPkField".to_string(),
            "NullifierPk".to_string(),
            "PrivateTxBlinding".to_string(),
        ];
        expected.extend(expected_escrow_terms_witness_keys("Terms"));
        expected.extend(utxo_proof_input_keys("Source"));
        expected.extend(utxo_proof_input_keys("EscrowUtxo"));
        expected.extend(utxo_proof_input_keys("Change"));

        assert_eq!(keys, expected.into_iter().collect::<HashSet<String>>());
    }
}

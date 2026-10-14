use std::collections::HashMap;

use zolana_client::ProofInputUtxo;
use zolana_gnark_ffi_prover::{decimal, utxo_proof_inputs, ProofInputMap};

use crate::{CircuitId, DvpProof, SwapDvpProofInput, PROVER};

/// The create_dvp witness. Its shape is the Go `create_dvp.Circuit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDvpProofInputs {
    /// `CreateDvpPublicInput::hash`, the only public input.
    pub public_input_hash: [u8; 32],
    pub private_tx_hash: [u8; 32],
    pub creator_id: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_authority_owner_hash: [u8; 32],
    pub terms: SwapDvpProofInput,
    /// Output 0, committed under the output tree's id.
    pub dvp_utxo: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
}

impl CreateDvpProofInputs {
    fn witness(&self) -> ProofInputMap {
        let scalars: [(&str, [u8; 32]); 6] = [
            ("PublicInputHash", self.public_input_hash),
            ("PrivateTxHash", self.private_tx_hash),
            ("CreatorID", self.creator_id),
            ("DvpID", self.dvp_id),
            ("DvpAuthorityOwnerHash", self.dvp_authority_owner_hash),
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
            .chain(utxo_proof_inputs(&self.dvp_utxo, "DvpUtxo"))
        {
            map.insert(key, value);
        }
        map
    }

    pub fn prove(&self) -> zolana_gnark_ffi_prover::Result<DvpProof> {
        Ok(PROVER
            .prove(CircuitId::CreateDvp, &self.witness())?
            .compress()?
            .into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zolana_gnark_ffi_prover::utxo_proof_input_keys;

    use super::*;
    use crate::{swap_dvp::expected_swap_dvp_witness_keys, DestinationProofInput};

    fn sample() -> CreateDvpProofInputs {
        let destination = DestinationProofInput {
            owner_hash: [1; 32],
            viewing_pk_hash: [2; 32],
        };
        CreateDvpProofInputs {
            public_input_hash: [1; 32],
            private_tx_hash: [2; 32],
            creator_id: [3; 32],
            dvp_id: [4; 32],
            dvp_authority_owner_hash: [5; 32],
            terms: SwapDvpProofInput {
                seller_id: [0; 32],
                buyer_id: [0; 32],
                settlement_authority_id: [0; 32],
                creator_id: [3; 32],
                delivery_asset: [0; 32],
                payment_asset: [0; 32],
                delivery_amount: 1,
                payment_amount: 2,
                expiry_timestamp: 3,
                has_earliest_settlement: false,
                earliest_settlement_timestamp: 0,
                nonce: 4,
                ref_string_hash: [0; 32],
                seller_settlement_destination: destination,
                buyer_settlement_destination: destination,
                seller_refund_destination: destination,
                buyer_refund_destination: destination,
                blinding: [0; 32],
            },
            dvp_utxo: ProofInputUtxo::default(),
            private_tx_blinding: [6; 32],
        }
    }

    #[test]
    fn witness_key_set_matches_circuit_fields() {
        let witness = sample().witness();
        let keys: HashSet<String> = witness.keys().cloned().collect();

        let mut expected: Vec<String> = [
            "PublicInputHash",
            "PrivateTxHash",
            "CreatorID",
            "DvpID",
            "DvpAuthorityOwnerHash",
            "PrivateTxBlinding",
        ]
        .map(String::from)
        .to_vec();
        expected.extend(expected_swap_dvp_witness_keys("Terms"));
        expected.extend(utxo_proof_input_keys("DvpUtxo"));

        assert_eq!(keys, expected.into_iter().collect::<HashSet<String>>());
    }
}

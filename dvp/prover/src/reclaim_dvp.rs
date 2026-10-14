use std::collections::HashMap;

use dvp_program::instructions::shared::u64_right_align;
use zolana_client::ProofInputUtxo;
use zolana_gnark_ffi_prover::{decimal, utxo_proof_inputs, ProofInputMap};

use crate::{CircuitId, DvpProof, ProveError, SwapDvpProofInput};

/// The reclaim_dvp witness. Its shape is the Go `reclaim_dvp.Circuit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimDvpProofInputs {
    /// `ReclaimDvpPublicInput::hash`, the only public input.
    pub public_input_hash: [u8; 32],
    pub private_tx_hash: [u8; 32],
    pub creator_id: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_authority_owner_hash: [u8; 32],
    pub signer_id: [u8; 32],
    pub terms: SwapDvpProofInput,
    /// `false` for the seller's delivery leg, `true` for the buyer's payment
    /// leg.
    pub leg: bool,
    /// Input 0, committed under the tree it is spent from.
    pub deposit: ProofInputUtxo,
    /// Output 0, committed under the output tree's id.
    pub refund: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
}

impl ReclaimDvpProofInputs {
    fn witness(&self) -> ProofInputMap {
        let scalars: [(&str, [u8; 32]); 8] = [
            ("PublicInputHash", self.public_input_hash),
            ("PrivateTxHash", self.private_tx_hash),
            ("CreatorID", self.creator_id),
            ("DvpID", self.dvp_id),
            ("DvpAuthorityOwnerHash", self.dvp_authority_owner_hash),
            ("SignerID", self.signer_id),
            ("Leg", u64_right_align(u64::from(self.leg))),
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
            .chain(utxo_proof_inputs(&self.deposit, "Deposit"))
            .chain(utxo_proof_inputs(&self.refund, "Refund"))
        {
            map.insert(key, value);
        }
        map
    }

    pub fn prove(&self) -> Result<DvpProof, ProveError> {
        DvpProof::prove(CircuitId::ReclaimDvp, &self.witness())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zolana_gnark_ffi_prover::utxo_proof_input_keys;

    use super::*;
    use crate::{swap_dvp::expected_swap_dvp_witness_keys, DestinationProofInput};

    fn sample() -> ReclaimDvpProofInputs {
        let destination = DestinationProofInput {
            owner_hash: [1; 32],
            viewing_pk_hash: [2; 32],
        };
        ReclaimDvpProofInputs {
            public_input_hash: [1; 32],
            private_tx_hash: [2; 32],
            creator_id: [3; 32],
            dvp_id: [4; 32],
            dvp_authority_owner_hash: [5; 32],
            signer_id: [6; 32],
            terms: SwapDvpProofInput {
                seller_id: [6; 32],
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
            leg: false,
            deposit: ProofInputUtxo::default(),
            refund: ProofInputUtxo::default(),
            private_tx_blinding: [7; 32],
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
            "SignerID",
            "Leg",
            "PrivateTxBlinding",
        ]
        .map(String::from)
        .to_vec();
        expected.extend(expected_swap_dvp_witness_keys("Terms"));
        expected.extend(utxo_proof_input_keys("Deposit"));
        expected.extend(utxo_proof_input_keys("Refund"));

        assert_eq!(keys, expected.into_iter().collect::<HashSet<String>>());
    }
}

use std::collections::HashMap;

use dvp_program::instructions::shared::u64_right_align;
use zolana_client::ProofInputUtxo;
use zolana_gnark_ffi_prover::{decimal, utxo_proof_inputs, ProofInputMap};

use crate::{CircuitId, DvpProof, ProveError, SwapDvpProofInput};

/// The settle_dvp witness. Its shape is the Go `settle_dvp.Circuit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleDvpProofInputs {
    /// `SettleDvpPublicInput::hash`, the only public input.
    pub public_input_hash: [u8; 32],
    pub private_tx_hash: [u8; 32],
    pub creator_id: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_authority_owner_hash: [u8; 32],
    pub settlement_authority_id: [u8; 32],
    /// Non-negative (D5), so it is the u64 the program right-aligns.
    pub current_timestamp: u64,
    pub terms: SwapDvpProofInput,
    /// Input 0, committed under the tree it is spent from.
    pub dvp_utxo: ProofInputUtxo,
    /// Input 1.
    pub delivery_deposit: ProofInputUtxo,
    /// Input 2.
    pub payment_deposit: ProofInputUtxo,
    /// Output 0: the delivery amount to the buyer's settlement destination.
    pub buyer_settlement: ProofInputUtxo,
    /// Output 1: the payment amount to the seller's settlement destination.
    pub seller_settlement: ProofInputUtxo,
    /// Output 2: the delivery surplus to the seller's refund destination.
    pub seller_refund: ProofInputUtxo,
    /// Output 3: the payment surplus to the buyer's refund destination.
    pub buyer_refund: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
}

impl SettleDvpProofInputs {
    fn witness(&self) -> ProofInputMap {
        let scalars: [(&str, [u8; 32]); 8] = [
            ("PublicInputHash", self.public_input_hash),
            ("PrivateTxHash", self.private_tx_hash),
            ("CreatorID", self.creator_id),
            ("DvpID", self.dvp_id),
            ("DvpAuthorityOwnerHash", self.dvp_authority_owner_hash),
            ("SettlementAuthorityID", self.settlement_authority_id),
            ("CurrentTimestamp", u64_right_align(self.current_timestamp)),
            ("PrivateTxBlinding", self.private_tx_blinding),
        ];
        let utxos: [(&str, &ProofInputUtxo); 7] = [
            ("DvpUtxo", &self.dvp_utxo),
            ("DeliveryDeposit", &self.delivery_deposit),
            ("PaymentDeposit", &self.payment_deposit),
            ("BuyerSettlement", &self.buyer_settlement),
            ("SellerSettlement", &self.seller_settlement),
            ("SellerRefund", &self.seller_refund),
            ("BuyerRefund", &self.buyer_refund),
        ];
        let mut map = HashMap::new();
        for (key, value) in scalars.iter() {
            map.insert(key.to_string(), vec![decimal(value)]);
        }
        for (key, value) in self.terms.witness_entries("Terms").into_iter().chain(
            utxos
                .into_iter()
                .flat_map(|(prefix, utxo)| utxo_proof_inputs(utxo, prefix)),
        ) {
            map.insert(key, value);
        }
        map
    }

    pub fn prove(&self) -> Result<DvpProof, ProveError> {
        DvpProof::prove(CircuitId::SettleDvp, &self.witness())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zolana_gnark_ffi_prover::utxo_proof_input_keys;

    use super::*;
    use crate::{swap_dvp::expected_swap_dvp_witness_keys, DestinationProofInput};

    fn sample() -> SettleDvpProofInputs {
        let destination = DestinationProofInput {
            owner_hash: [1; 32],
            viewing_pk_hash: [2; 32],
        };
        SettleDvpProofInputs {
            public_input_hash: [1; 32],
            private_tx_hash: [2; 32],
            creator_id: [3; 32],
            dvp_id: [4; 32],
            dvp_authority_owner_hash: [5; 32],
            settlement_authority_id: [6; 32],
            current_timestamp: 2,
            terms: SwapDvpProofInput {
                seller_id: [0; 32],
                buyer_id: [0; 32],
                settlement_authority_id: [6; 32],
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
            delivery_deposit: ProofInputUtxo::default(),
            payment_deposit: ProofInputUtxo::default(),
            buyer_settlement: ProofInputUtxo::default(),
            seller_settlement: ProofInputUtxo::default(),
            seller_refund: ProofInputUtxo::default(),
            buyer_refund: ProofInputUtxo::default(),
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
            "SettlementAuthorityID",
            "CurrentTimestamp",
            "PrivateTxBlinding",
        ]
        .map(String::from)
        .to_vec();
        expected.extend(expected_swap_dvp_witness_keys("Terms"));
        for prefix in [
            "DvpUtxo",
            "DeliveryDeposit",
            "PaymentDeposit",
            "BuyerSettlement",
            "SellerSettlement",
            "SellerRefund",
            "BuyerRefund",
        ] {
            expected.extend(utxo_proof_input_keys(prefix));
        }

        assert_eq!(keys, expected.into_iter().collect::<HashSet<String>>());
    }
}

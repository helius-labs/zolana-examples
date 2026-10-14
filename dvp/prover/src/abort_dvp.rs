use std::collections::HashMap;

use dvp_program::{instructions::shared::u64_right_align, ABORT_MODE_CANCEL, ABORT_MODE_REJECT};
use zolana_client::ProofInputUtxo;
use zolana_gnark_ffi_prover::{decimal, utxo_proof_inputs, ProofInputMap};

use crate::{CircuitId, DvpProof, ProveError, SwapDvpProofInput};

/// The abort_dvp witness, shared by cancel_dvp and reject_dvp. Its shape is
/// the Go `abort_dvp.Circuit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortDvpProofInputs {
    /// `AbortDvpPublicInput::hash`, the only public input.
    pub public_input_hash: [u8; 32],
    pub private_tx_hash: [u8; 32],
    pub creator_id: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_authority_owner_hash: [u8; 32],
    pub signer_id: [u8; 32],
    /// The public `Mode`, which picks the signer rule.
    pub mode: AbortMode,
    pub terms: SwapDvpProofInput,
    /// Reject mode: `false` for the seller, `true` for the buyer.
    pub leg: bool,
    /// Input slot 1 holds the payment leg and slot 2 the delivery leg.
    pub payment_first: bool,
    /// Input slot 1 is padding; then slot 2 is too.
    pub padding1: bool,
    pub padding2: bool,
    /// Input 0, committed under the tree it is spent from.
    pub dvp_utxo: ProofInputUtxo,
    /// Input 1: a deposit, or the dummy's witness when `padding1`.
    pub slot1: ProofInputUtxo,
    /// Input 2: a deposit, or the dummy's witness when `padding2`.
    pub slot2: ProofInputUtxo,
    /// Output 0: the delivery deposit (or 0) to the seller's refund
    /// destination.
    pub seller_refund: ProofInputUtxo,
    /// Output 1: the payment deposit (or 0) to the buyer's refund
    /// destination.
    pub buyer_refund: ProofInputUtxo,
    pub private_tx_blinding: [u8; 32],
}

/// Which instruction aborts the DvP; the abort proof's public `mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortMode {
    /// cancel_dvp: the settlement authority aborts.
    Cancel,
    /// reject_dvp: the seller or the buyer aborts.
    Reject,
}

impl AbortMode {
    /// `ABORT_MODE_CANCEL` or `ABORT_MODE_REJECT`, as the program puts it in
    /// the public input.
    pub fn value(self) -> u64 {
        u64::from(match self {
            Self::Cancel => ABORT_MODE_CANCEL,
            Self::Reject => ABORT_MODE_REJECT,
        })
    }
}

impl AbortDvpProofInputs {
    fn witness(&self) -> ProofInputMap {
        let bit = |value: bool| u64_right_align(u64::from(value));
        let scalars: [(&str, [u8; 32]); 12] = [
            ("PublicInputHash", self.public_input_hash),
            ("PrivateTxHash", self.private_tx_hash),
            ("CreatorID", self.creator_id),
            ("DvpID", self.dvp_id),
            ("DvpAuthorityOwnerHash", self.dvp_authority_owner_hash),
            ("SignerID", self.signer_id),
            ("Mode", u64_right_align(self.mode.value())),
            ("Leg", bit(self.leg)),
            ("PaymentFirst", bit(self.payment_first)),
            ("Padding1", bit(self.padding1)),
            ("Padding2", bit(self.padding2)),
            ("PrivateTxBlinding", self.private_tx_blinding),
        ];
        let utxos: [(&str, &ProofInputUtxo); 5] = [
            ("DvpUtxo", &self.dvp_utxo),
            ("Slot1", &self.slot1),
            ("Slot2", &self.slot2),
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
        DvpProof::prove(CircuitId::AbortDvp, &self.witness())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zolana_gnark_ffi_prover::utxo_proof_input_keys;

    use super::*;
    use crate::{swap_dvp::expected_swap_dvp_witness_keys, DestinationProofInput};

    fn sample() -> AbortDvpProofInputs {
        let destination = DestinationProofInput {
            owner_hash: [1; 32],
            viewing_pk_hash: [2; 32],
        };
        AbortDvpProofInputs {
            public_input_hash: [1; 32],
            private_tx_hash: [2; 32],
            creator_id: [3; 32],
            dvp_id: [4; 32],
            dvp_authority_owner_hash: [5; 32],
            signer_id: [6; 32],
            mode: AbortMode::Reject,
            terms: SwapDvpProofInput {
                seller_id: [0; 32],
                buyer_id: [6; 32],
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
            leg: true,
            payment_first: true,
            padding1: false,
            padding2: true,
            dvp_utxo: ProofInputUtxo::default(),
            slot1: ProofInputUtxo::default(),
            slot2: ProofInputUtxo::default(),
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
            "SignerID",
            "Mode",
            "Leg",
            "PaymentFirst",
            "Padding1",
            "Padding2",
            "PrivateTxBlinding",
        ]
        .map(String::from)
        .to_vec();
        expected.extend(expected_swap_dvp_witness_keys("Terms"));
        for prefix in ["DvpUtxo", "Slot1", "Slot2", "SellerRefund", "BuyerRefund"] {
            expected.extend(utxo_proof_input_keys(prefix));
        }

        assert_eq!(keys, expected.into_iter().collect::<HashSet<String>>());
    }
}

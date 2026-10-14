use dvp_program::instructions::shared::u64_right_align;
use zolana_gnark_ffi_prover::decimal;

/// One shielded destination as the circuits see it: the compressed shielded
/// address `Poseidon(OwnerHash, ViewingPkHash)` split into its two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationProofInput {
    pub owner_hash: [u8; 32],
    /// `hash_bytes(viewing_pk[33])`.
    pub viewing_pk_hash: [u8; 32],
}

/// The `SwapDvp` terms as field elements, in the shape of `dvpterms.SwapDvp`.
/// Party fields are `solana_owner_identity(pk)`, assets are `hash_bytes(mint)`,
/// and timestamps are non-negative (D5) so they fit a u64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapDvpProofInput {
    pub seller_id: [u8; 32],
    pub buyer_id: [u8; 32],
    pub settlement_authority_id: [u8; 32],
    pub creator_id: [u8; 32],
    pub delivery_asset: [u8; 32],
    pub payment_asset: [u8; 32],
    pub delivery_amount: u64,
    pub payment_amount: u64,
    pub expiry_timestamp: u64,
    pub has_earliest_settlement: bool,
    /// 0 when `has_earliest_settlement` is false.
    pub earliest_settlement_timestamp: u64,
    pub nonce: u64,
    /// `hash_bytes(ref_string[64])`.
    pub ref_string_hash: [u8; 32],
    pub seller_settlement_destination: DestinationProofInput,
    pub buyer_settlement_destination: DestinationProofInput,
    pub seller_refund_destination: DestinationProofInput,
    pub buyer_refund_destination: DestinationProofInput,
    pub blinding: [u8; 32],
}

impl SwapDvpProofInput {
    pub fn witness_entries(&self, prefix: &str) -> Vec<(String, Vec<String>)> {
        let scalars: [(&str, [u8; 32]); 14] = [
            ("SellerID", self.seller_id),
            ("BuyerID", self.buyer_id),
            ("SettlementAuthorityID", self.settlement_authority_id),
            ("CreatorID", self.creator_id),
            ("DeliveryAsset", self.delivery_asset),
            ("PaymentAsset", self.payment_asset),
            ("DeliveryAmount", u64_right_align(self.delivery_amount)),
            ("PaymentAmount", u64_right_align(self.payment_amount)),
            ("ExpiryTimestamp", u64_right_align(self.expiry_timestamp)),
            (
                "HasEarliestSettlement",
                u64_right_align(u64::from(self.has_earliest_settlement)),
            ),
            (
                "EarliestSettlementTimestamp",
                u64_right_align(self.earliest_settlement_timestamp),
            ),
            ("Nonce", u64_right_align(self.nonce)),
            ("RefStringHash", self.ref_string_hash),
            ("Blinding", self.blinding),
        ];
        let destinations = [
            (
                "SellerSettlementDestination",
                &self.seller_settlement_destination,
            ),
            (
                "BuyerSettlementDestination",
                &self.buyer_settlement_destination,
            ),
            ("SellerRefundDestination", &self.seller_refund_destination),
            ("BuyerRefundDestination", &self.buyer_refund_destination),
        ];
        scalars
            .iter()
            .map(|(suffix, value)| (format!("{prefix}_{suffix}"), vec![decimal(value)]))
            .chain(destinations.iter().flat_map(|(name, destination)| {
                [
                    ("OwnerHash", &destination.owner_hash),
                    ("ViewingPkHash", &destination.viewing_pk_hash),
                ]
                .map(|(half, value)| (format!("{prefix}_{name}_{half}"), vec![decimal(value)]))
            }))
            .collect()
    }
}

/// The witness keys one `dvpterms.SwapDvp` prefix must produce: a second,
/// hand-written copy of the Go field names, so the key-set tests catch a
/// builder key edited by mistake without the proving keys. They do not read
/// the Go structs; the gnark bridge's exact key check when proving (every
/// sdk circuit test) is what ties the keys to the circuits.
#[cfg(test)]
pub(crate) fn expected_swap_dvp_witness_keys(prefix: &str) -> Vec<String> {
    let scalars = [
        "SellerID",
        "BuyerID",
        "SettlementAuthorityID",
        "CreatorID",
        "DeliveryAsset",
        "PaymentAsset",
        "DeliveryAmount",
        "PaymentAmount",
        "ExpiryTimestamp",
        "HasEarliestSettlement",
        "EarliestSettlementTimestamp",
        "Nonce",
        "RefStringHash",
        "Blinding",
    ];
    let destinations = [
        "SellerSettlementDestination",
        "BuyerSettlementDestination",
        "SellerRefundDestination",
        "BuyerRefundDestination",
    ];
    scalars
        .iter()
        .map(|suffix| format!("{prefix}_{suffix}"))
        .chain(destinations.iter().flat_map(|name| {
            ["OwnerHash", "ViewingPkHash"].map(|half| format!("{prefix}_{name}_{half}"))
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn witness_keys_match_go_fields() {
        let destination = DestinationProofInput {
            owner_hash: [1; 32],
            viewing_pk_hash: [2; 32],
        };
        let input = SwapDvpProofInput {
            seller_id: [0; 32],
            buyer_id: [0; 32],
            settlement_authority_id: [0; 32],
            creator_id: [0; 32],
            delivery_asset: [0; 32],
            payment_asset: [0; 32],
            delivery_amount: 1,
            payment_amount: 2,
            expiry_timestamp: 3,
            has_earliest_settlement: true,
            earliest_settlement_timestamp: 2,
            nonce: 4,
            ref_string_hash: [0; 32],
            seller_settlement_destination: destination,
            buyer_settlement_destination: destination,
            seller_refund_destination: destination,
            buyer_refund_destination: destination,
            blinding: [0; 32],
        };
        let entries = input.witness_entries("Terms");
        let keys: BTreeSet<String> = entries.iter().map(|(key, _)| key.clone()).collect();
        let expected: BTreeSet<String> = expected_swap_dvp_witness_keys("Terms")
            .into_iter()
            .collect();
        assert_eq!(keys, expected);
        assert_eq!(entries.len(), expected.len());
        assert!(entries.iter().all(|(_, values)| values.len() == 1));
    }
}

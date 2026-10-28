use zolana_program::circuit::hash_bytes;
use zolana_program::{
    input, poseidon, require, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress,
};

#[input]
#[derive(ClientInputs)]
pub struct DvpDestination {
    pub address: ShieldedAddress,
    pub viewing_key_hash: Field,
}

impl DvpDestination {
    pub fn hash(&self) -> Result<Field, CircuitError> {
        require_eq(
            self.viewing_key_hash,
            hash_bytes(self.address.viewing_pubkey())?,
        );
        poseidon(&[self.address.owner().hash()?, self.viewing_key_hash])
    }
}

#[input]
pub struct SwapDvpTerms {
    pub seller_identity: Field,
    pub buyer_identity: Field,
    pub settlement_authority_identity: Field,
    pub creator_identity: Field,
    pub delivery_mint: [u8; 32],
    pub payment_mint: [u8; 32],
    pub delivery_amount: u64,
    pub payment_amount: u64,
    pub expiry_timestamp: u64,
    pub has_earliest_settlement: bool,
    pub earliest_settlement_timestamp: u64,
    pub nonce: u64,
    pub reference_hash: Field,
    pub seller_settlement_destination: DvpDestination,
    pub buyer_settlement_destination: DvpDestination,
    pub seller_refund_destination: DvpDestination,
    pub buyer_refund_destination: DvpDestination,
    pub blinding: Field,
}

impl SwapDvpTerms {
    pub fn hash(&self) -> Result<Field, CircuitError> {
        let parties = poseidon(&[
            self.seller_identity,
            self.buyer_identity,
            self.settlement_authority_identity,
            self.creator_identity,
        ])?;
        let legs = poseidon(&[
            hash_bytes(&self.delivery_mint)?,
            hash_bytes(&self.payment_mint)?,
            Field::from(self.delivery_amount),
            Field::from(self.payment_amount),
        ])?;
        let schedule = poseidon(&[
            Field::from(self.expiry_timestamp),
            Field::from(self.has_earliest_settlement),
            Field::from(self.earliest_settlement_timestamp),
            Field::from(self.nonce),
        ])?;
        let destinations = poseidon(&[
            self.seller_settlement_destination.hash()?,
            self.buyer_settlement_destination.hash()?,
            self.seller_refund_destination.hash()?,
            self.buyer_refund_destination.hash()?,
        ])?;
        poseidon(&[
            parties,
            legs,
            schedule,
            destinations,
            self.reference_hash,
            self.blinding,
        ])
    }

    pub fn assert_dvp(&self, dvp_id: &Field, creator: &Field) -> Result<(), CircuitError> {
        require(self.expiry_timestamp <= i64::MAX as u64);
        require(self.earliest_settlement_timestamp <= i64::MAX as u64);
        require(self.has_earliest_settlement || self.earliest_settlement_timestamp == 0);
        require_eq(self.creator_identity, *creator);
        require_eq(self.hash()?, *dvp_id);
        Ok(())
    }

    pub fn assert_valid_trade(&self) -> Result<(), CircuitError> {
        require(self.delivery_amount != 0);
        require(self.payment_amount != 0);
        require(self.delivery_mint != self.payment_mint);
        require(
            !self.has_earliest_settlement
                || self.earliest_settlement_timestamp <= self.expiry_timestamp,
        );
        Ok(())
    }
}

#[cfg(all(not(xark), not(xark_client)))]
mod client {
    use super::*;
    use zolana_program::{ClientError, InputRecords};

    #[cfg_attr(
        feature = "wasm",
        derive(serde::Deserialize, tsify::Tsify),
        serde(rename_all = "camelCase", deny_unknown_fields),
        tsify(large_number_types_as_bigints)
    )]
    pub struct SwapDvpTermsClient {
        pub seller_identity: String,
        pub buyer_identity: String,
        pub settlement_authority_identity: String,
        pub creator_identity: String,
        #[cfg_attr(feature = "wasm", tsify(type = "Mint"))]
        pub delivery_mint: zolana_transaction::Mint,
        #[cfg_attr(feature = "wasm", tsify(type = "Mint"))]
        pub payment_mint: zolana_transaction::Mint,
        pub delivery_amount: u64,
        pub payment_amount: u64,
        pub expiry_timestamp: u64,
        pub has_earliest_settlement: bool,
        pub earliest_settlement_timestamp: u64,
        pub nonce: u64,
        pub reference_hash: String,
        #[cfg_attr(
            feature = "wasm",
            tsify(type = "DvpDestinationClient"),
            serde(deserialize_with = "zolana_program::wasm::deserialize_client_input")
        )]
        pub seller_settlement_destination: DvpDestinationClient,
        #[cfg_attr(
            feature = "wasm",
            tsify(type = "DvpDestinationClient"),
            serde(deserialize_with = "zolana_program::wasm::deserialize_client_input")
        )]
        pub buyer_settlement_destination: DvpDestinationClient,
        #[cfg_attr(
            feature = "wasm",
            tsify(type = "DvpDestinationClient"),
            serde(deserialize_with = "zolana_program::wasm::deserialize_client_input")
        )]
        pub seller_refund_destination: DvpDestinationClient,
        #[cfg_attr(
            feature = "wasm",
            tsify(type = "DvpDestinationClient"),
            serde(deserialize_with = "zolana_program::wasm::deserialize_client_input")
        )]
        pub buyer_refund_destination: DvpDestinationClient,
        pub blinding: String,
    }

    impl ClientInputs for SwapDvpTerms {
        type Client = SwapDvpTermsClient;
        fn from_client(
            client: Self::Client,
            records: &mut InputRecords,
        ) -> Result<Self, ClientError> {
            let field = |value: &str| {
                zolana_program::from_decimal(value)
                    .map_err(|error| ClientError::InvalidInput(error.to_string()))
            };
            Ok(Self {
                seller_identity: field(&client.seller_identity)?,
                buyer_identity: field(&client.buyer_identity)?,
                settlement_authority_identity: field(&client.settlement_authority_identity)?,
                creator_identity: field(&client.creator_identity)?,
                delivery_mint: records.asset(client.delivery_mint),
                payment_mint: records.asset(client.payment_mint),
                delivery_amount: client.delivery_amount,
                payment_amount: client.payment_amount,
                expiry_timestamp: client.expiry_timestamp,
                has_earliest_settlement: client.has_earliest_settlement,
                earliest_settlement_timestamp: client.earliest_settlement_timestamp,
                nonce: client.nonce,
                reference_hash: field(&client.reference_hash)?,
                seller_settlement_destination: DvpDestination::from_client(
                    client.seller_settlement_destination,
                    records,
                )?,
                buyer_settlement_destination: DvpDestination::from_client(
                    client.buyer_settlement_destination,
                    records,
                )?,
                seller_refund_destination: DvpDestination::from_client(
                    client.seller_refund_destination,
                    records,
                )?,
                buyer_refund_destination: DvpDestination::from_client(
                    client.buyer_refund_destination,
                    records,
                )?,
                blinding: field(&client.blinding)?,
            })
        }
    }
}
#[cfg(all(not(xark), not(xark_client)))]
pub use client::SwapDvpTermsClient;

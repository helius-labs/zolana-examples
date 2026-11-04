use spr_program::circuit::hash_bytes;
use spr_program::{
    input, poseidon, require, require_eq, Asset, CircuitError, ClientInputs, Field, ShieldedAddress,
};

#[input]
#[derive(ClientInputs)]
pub struct DvpDestination {
    pub address: ShieldedAddress,
    pub viewing_key_hash: Field,
}

impl DvpDestination {
    pub fn hash(&self) -> Result<Field, CircuitError> {
        poseidon(&[self.address.owner().hash()?, self.viewing_key_hash])
    }
}

#[input]
#[derive(ClientInputs)]
pub struct SwapDvpTerms {
    pub seller_identity: Field,
    pub buyer_identity: Field,
    pub settlement_authority_identity: Field,
    pub creator_identity: Field,
    pub delivery_mint: Asset,
    pub payment_mint: Asset,
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

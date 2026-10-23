use zolana_keypair::ShieldedAddress;
use zolana_program::{
    circuit::{poseidon, zero, Assert, CircuitVar, DataHash},
    conversion::ProofInput,
    CircuitError,
};
use zolana_transaction::Mint;

#[derive(Clone, Debug, ProofInput)]
pub struct DvpDestination {
    pub address: ShieldedAddress,
    pub viewing_key_hash: [u8; 32],
}

impl DvpDestinationCircuit {
    pub fn hash(&self) -> Result<CircuitVar, CircuitError> {
        poseidon(&[self.address.hash()?, self.viewing_key_hash.clone()])
    }
}

#[derive(Clone, Debug, ProofInput)]
pub struct SwapDvpTerms {
    pub seller_identity: [u8; 32],
    pub buyer_identity: [u8; 32],
    pub settlement_authority_identity: [u8; 32],
    pub creator_identity: [u8; 32],
    pub delivery_mint: Mint,
    pub payment_mint: Mint,
    pub delivery_amount: u64,
    pub payment_amount: u64,
    pub expiry_timestamp: u64,
    pub has_earliest_settlement: bool,
    pub earliest_settlement_timestamp: u64,
    pub nonce: u64,
    pub reference_hash: [u8; 32],
    pub seller_settlement_destination: DvpDestination,
    pub buyer_settlement_destination: DvpDestination,
    pub seller_refund_destination: DvpDestination,
    pub buyer_refund_destination: DvpDestination,
    pub blinding: [u8; 32],
}

impl SwapDvpTermsCircuit {
    pub fn hash(&self) -> Result<CircuitVar, CircuitError> {
        let parties = poseidon(&[
            self.seller_identity.clone(),
            self.buyer_identity.clone(),
            self.settlement_authority_identity.clone(),
            self.creator_identity.clone(),
        ])?;
        let legs = poseidon(&[
            self.delivery_mint.hash()?,
            self.payment_mint.hash()?,
            self.delivery_amount.var(),
            self.payment_amount.var(),
        ])?;
        let schedule = poseidon(&[
            self.expiry_timestamp.var(),
            self.has_earliest_settlement.var(),
            self.earliest_settlement_timestamp.var(),
            self.nonce.var(),
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
            self.reference_hash.clone(),
            self.blinding.clone(),
        ])
    }

    pub fn assert_dvp(
        &self,
        dvp_id: &CircuitVar,
        creator: &CircuitVar,
    ) -> Result<(), CircuitError> {
        self.expiry_timestamp.var().check_bits(63)?;
        self.earliest_settlement_timestamp.var().check_bits(63)?;
        self.earliest_settlement_timestamp.var().assert_equal_if(
            &zero(),
            &self.has_earliest_settlement.not(),
            "an unset earliest settlement timestamp is zero",
        )?;
        self.creator_identity
            .assert_equal(creator, "the creator matches the terms")?;
        self.hash()?
            .assert_equal(dvp_id, "the DvP id matches the terms")
    }

    pub fn assert_valid_trade(&self) -> Result<(), CircuitError> {
        self.delivery_amount
            .assert_not_zero("the delivery leg moves value")?;
        self.payment_amount
            .assert_not_zero("the payment leg moves value")?;
        self.delivery_mint.hash()?.assert_not_equal(
            &self.payment_mint.hash()?,
            "the trade uses different assets",
        )?;
        self.earliest_settlement_timestamp
            .is_less_or_equal(&self.expiry_timestamp)?
            .assert_true_if(
                &self.has_earliest_settlement,
                "the settlement window is ordered",
            )
    }
}

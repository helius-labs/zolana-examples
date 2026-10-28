use super::{
    shared::{assert_blinding_seed, close_dvp_state},
    SwapDvpTerms,
};
use zolana_program::circuit::Utxo;
use zolana_program::{
    circuit::{CheckedTransaction, ConfidentialTransaction, TokenUtxos, UtxoTrait},
    input, require, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext,
    ZkProgram,
};

#[input]
#[derive(ZkProgram)]
pub struct SettleDvp {
    pub private: SettleDvpPrivateInputs,
    pub public: SettleDvpPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct SettleDvpPrivateInputs {
    pub tx_context: TxContext,
    #[client(nested)]
    pub terms: SwapDvpTerms,
    pub dvp_utxo: Utxo,
    pub delivery_deposit: Utxo,
    pub payment_deposit: Utxo,
}

#[input]
#[derive(ClientInputs)]
pub struct SettleDvpPublicInputs {
    pub creator_identity: Field,
    pub dvp_id: Field,
    pub dvp_owner: ShieldedAddress,
    pub settlement_authority_identity: Field,
    pub current_timestamp: u64,
}

impl SettleDvp {
    /// 1. Check terms and creator.
    /// 2. Check the settlement authority is the signer.
    /// 3. Check the settlement time bounds.
    /// 4. Check the settlement seed and spend the DvP UTXO.
    /// 5. Spend the delivery deposit.
    /// 6. Spend the payment deposit.
    /// 7. Create the buyer's delivery settlement.
    /// 8. Create the seller's payment settlement.
    /// 9. Create the seller's delivery change.
    /// 10. Create the buyer's payment change.
    /// 11. Send the agreed delivery amount.
    /// 12. Send the agreed payment amount.
    /// 13. Refund the delivery surplus.
    /// 14. Refund the payment surplus.
    /// 15. Check the transaction.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let terms = &private.terms;

        // 1. Check terms and creator.
        terms.assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check the settlement authority is the signer.
        require_eq(
            terms.settlement_authority_identity,
            public.settlement_authority_identity,
        );

        // 3. Check the settlement time bounds.
        require(public.current_timestamp <= i64::MAX as u64);
        require(public.current_timestamp <= terms.expiry_timestamp);
        require(
            !terms.has_earliest_settlement
                || terms.earliest_settlement_timestamp <= public.current_timestamp,
        );

        // 4. Check the settlement seed and spend the DvP UTXO.
        assert_blinding_seed(&private.tx_context, crate::SETTLEMENT_TAG, &terms.blinding)?;
        let dvp_utxo = close_dvp_state(&private.dvp_utxo, &public.dvp_owner, &public.dvp_id)?;

        // 5. Spend the delivery deposit.
        let mut delivery_deposit =
            TokenUtxos::new_mut(core::array::from_ref(&private.delivery_deposit))?.close();
        require_eq(delivery_deposit.owner(), *public.dvp_owner.owner());
        require_eq(delivery_deposit.asset(), terms.delivery_mint);

        // 6. Spend the payment deposit.
        let mut payment_deposit =
            TokenUtxos::new_mut(core::array::from_ref(&private.payment_deposit))?.close();
        require_eq(payment_deposit.owner(), *public.dvp_owner.owner());
        require_eq(payment_deposit.asset(), terms.payment_mint);

        // 7. Create the buyer's delivery settlement.
        let mut delivery_settlement = TokenUtxos::new_init(
            &terms.buyer_settlement_destination.address,
            &terms.delivery_mint,
        );

        // 8. Create the seller's payment settlement.
        let mut payment_settlement = TokenUtxos::new_init(
            &terms.seller_settlement_destination.address,
            &terms.payment_mint,
        );

        // 9. Create the seller's delivery change.
        let mut delivery_change = TokenUtxos::new_init(
            &terms.seller_refund_destination.address,
            &terms.delivery_mint,
        )
        .with_zero_output();

        // 10. Create the buyer's payment change.
        let mut payment_change =
            TokenUtxos::new_init(&terms.buyer_refund_destination.address, &terms.payment_mint)
                .with_zero_output();

        // 11. Send the agreed delivery amount.
        delivery_deposit.transfer(&mut delivery_settlement, &terms.delivery_amount)?;

        // 12. Send the agreed payment amount.
        payment_deposit.transfer(&mut payment_settlement, &terms.payment_amount)?;

        // 13. Refund the delivery surplus.
        delivery_deposit.transfer_all(&mut delivery_change)?;

        // 14. Refund the payment surplus.
        payment_deposit.transfer_all(&mut payment_change)?;

        // 15. Check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(dvp_utxo)
            .with(delivery_deposit)
            .with(payment_deposit)
            .with(delivery_settlement)
            .with(payment_settlement)
            .with(delivery_change)
            .with(payment_change)
            .check()
    }
}

impl zolana_program::circuit::PublicInputs for SettleDvpPublicInputs {
    fn hash(&self, transaction_hash: &Field) -> Result<Field, CircuitError> {
        zolana_program::poseidon(&[
            self.creator_identity,
            self.dvp_id,
            self.dvp_owner.owner().hash()?,
            self.settlement_authority_identity,
            Field::from(self.current_timestamp),
            *transaction_hash,
        ])
    }
}

#[cfg(all(feature = "r1cs-export", not(xark), not(xark_client)))]
impl zolana_program::build::PublicInputLayout for SettleDvpPublicInputs {
    fn layout() -> Vec<(&'static str, zolana_program::build::PublicFieldKind)> {
        use zolana_program::build::PublicFieldKind;
        vec![
            ("creator_identity", PublicFieldKind::Hash),
            ("dvp_id", PublicFieldKind::Hash),
            ("dvp_owner", PublicFieldKind::Hash),
            ("settlement_authority_identity", PublicFieldKind::Hash),
            ("current_timestamp", PublicFieldKind::Integer { bits: 64 }),
        ]
    }
}

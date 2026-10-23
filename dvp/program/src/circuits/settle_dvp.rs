use super::{
    shared::{assert_blinding_seed, close_dvp_state},
    SwapDvpTerms,
};
use zolana_keypair::ShieldedAddress;
use zolana_program::{
    circuit::{
        Assert, CheckedTransaction, Circuit, ConfidentialTransaction, PublicInputs, TokenUtxos,
        UtxoTrait,
    },
    conversion::ProofInput,
    CircuitError, TxContext, ZkProgramCircuit,
};
use zolana_transaction::WalletUtxo;

#[derive(Clone, ZkProgramCircuit)]
pub struct SettleDvp {
    pub private: SettleDvpPrivateInputs,
    pub public: SettleDvpPublicInputs,
}

#[derive(Clone, ProofInput)]
pub struct SettleDvpPrivateInputs {
    pub tx_context: TxContext,
    pub terms: SwapDvpTerms,
    pub dvp_utxo: WalletUtxo,
    pub delivery_deposit: WalletUtxo,
    pub payment_deposit: WalletUtxo,
}

#[derive(Clone, PublicInputs)]
pub struct SettleDvpPublicInputs {
    pub creator_identity: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_owner: ShieldedAddress,
    pub settlement_authority_identity: [u8; 32],
    pub current_timestamp: u64,
}

impl Circuit for <SettleDvp as ProofInput>::Circuit {
    type Output = CheckedTransaction;

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
    fn circuit(&self) -> Result<Self::Output, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let terms = &private.terms;

        // 1. Check terms and creator.
        terms.assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check the settlement authority is the signer.
        terms.settlement_authority_identity.assert_equal(
            &public.settlement_authority_identity,
            "only the settlement authority settles",
        )?;

        // 3. Check the settlement time bounds.
        public.current_timestamp.var().check_bits(63)?;
        public
            .current_timestamp
            .assert_less_or_equal(&terms.expiry_timestamp, "settlement precedes expiry")?;
        terms
            .earliest_settlement_timestamp
            .is_less_or_equal(&public.current_timestamp)?
            .assert_true_if(
                &terms.has_earliest_settlement,
                "settlement follows the earliest settlement time",
            )?;

        // 4. Check the settlement seed and spend the DvP UTXO.
        assert_blinding_seed(&private.tx_context, crate::SETTLEMENT_TAG, &terms.blinding)?;
        let dvp_utxo = close_dvp_state(&private.dvp_utxo, &public.dvp_owner, &public.dvp_id)?;

        // 5. Spend the delivery deposit.
        let mut delivery_deposit =
            TokenUtxos::new_mut(core::array::from_ref(&private.delivery_deposit))?.close();
        delivery_deposit
            .owner()
            .assert_equal(&public.dvp_owner, "the PDA owns the delivery deposit")?;
        delivery_deposit
            .asset()
            .assert_equal(&terms.delivery_mint, "the delivery mint matches the terms")?;

        // 6. Spend the payment deposit.
        let mut payment_deposit =
            TokenUtxos::new_mut(core::array::from_ref(&private.payment_deposit))?.close();
        payment_deposit
            .owner()
            .assert_equal(&public.dvp_owner, "the PDA owns the payment deposit")?;
        payment_deposit
            .asset()
            .assert_equal(&terms.payment_mint, "the payment mint matches the terms")?;

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

use zolana_keypair::ShieldedAddress;
use zolana_program::{
    circuit::{
        Assert, CheckedTransaction, Circuit, ConfidentialTransaction, PublicInputs, TokenUtxos,
        UtxoTrait,
    },
    conversion::ProofInput,
    CircuitError, TxContext, ZkProgram,
};
use zolana_transaction::WalletUtxo;

use super::{
    shared::{assert_blinding_seed, close_dvp_state},
    SwapDvpTerms,
};

#[derive(Clone, ZkProgram)]
pub struct AbortDvp {
    pub private: AbortDvpPrivateInputs,
    pub public: AbortDvpPublicInputs,
}

#[derive(Clone, ProofInput)]
pub struct AbortDvpPrivateInputs {
    pub tx_context: TxContext,
    pub terms: SwapDvpTerms,
    pub dvp_utxo: WalletUtxo,
    pub delivery_deposit: WalletUtxo,
    pub payment_deposit: WalletUtxo,
    pub buyer_rejects: bool,
}

#[derive(Clone, PublicInputs)]
pub struct AbortDvpPublicInputs {
    pub creator_identity: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_owner: ShieldedAddress,
    pub signer_identity: [u8; 32],
    pub reject: bool,
}

impl Circuit for <AbortDvp as ProofInput>::Circuit {
    type Output = CheckedTransaction;

    /// 1. Check terms and creator.
    /// 2. Check the settlement authority for cancel or seller/buyer for reject.
    /// 3. Check the abort seed and spend the DvP UTXO.
    /// 4. Spend any delivery deposit.
    /// 5. Spend any payment deposit.
    /// 6. Create the seller's refund, including zero.
    /// 7. Create the buyer's refund, including zero.
    /// 8. Refund the delivery deposit to the seller.
    /// 9. Refund the payment deposit to the buyer.
    /// 10. Check the transaction with real deposits before padding.
    fn circuit(&self) -> Result<Self::Output, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let terms = &private.terms;

        // 1. Check terms and creator.
        terms.assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check the settlement authority for cancel or seller/buyer for reject.
        let party = private
            .buyer_rejects
            .select(&terms.buyer_identity, &terms.seller_identity);
        public
            .reject
            .select(&party, &terms.settlement_authority_identity)
            .assert_equal(
                &public.signer_identity,
                "the instruction requires its authorized signer",
            )?;

        // 3. Check the abort seed and spend the DvP UTXO.
        assert_blinding_seed(&private.tx_context, crate::ABORT_TAG, &terms.blinding)?;
        let dvp_utxo = close_dvp_state(&private.dvp_utxo, &public.dvp_owner, &public.dvp_id)?;

        // 4. Spend any delivery deposit.
        let mut delivery_deposit = TokenUtxos::new_optional_close(
            &private.delivery_deposit,
            &public.dvp_owner,
            &terms.delivery_mint,
        )?;

        // 5. Spend any payment deposit.
        let mut payment_deposit = TokenUtxos::new_optional_close(
            &private.payment_deposit,
            &public.dvp_owner,
            &terms.payment_mint,
        )?;

        // 6. Create the seller's refund, including zero.
        let mut seller_refund = TokenUtxos::new_init(
            &terms.seller_refund_destination.address,
            &terms.delivery_mint,
        )
        .with_zero_output();

        // 7. Create the buyer's refund, including zero.
        let mut buyer_refund =
            TokenUtxos::new_init(&terms.buyer_refund_destination.address, &terms.payment_mint)
                .with_zero_output();

        // 8. Refund the delivery deposit to the seller.
        delivery_deposit.transfer_all(&mut seller_refund)?;

        // 9. Refund the payment deposit to the buyer.
        payment_deposit.transfer_all(&mut buyer_refund)?;

        // 10. Check the transaction with real deposits before padding.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with_input_padding()
            .with_hashed_data_utxo(dvp_utxo)
            .with_optional_token_utxos([delivery_deposit, payment_deposit])
            .with_token_utxos(seller_refund)
            .with_token_utxos(buyer_refund)
            .check()
    }
}

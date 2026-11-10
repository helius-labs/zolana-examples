use super::SwapDvpTerms;
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

#[derive(Clone, ZkProgram)]
pub struct ReclaimDvp {
    pub private: ReclaimDvpPrivateInputs,
    pub public: ReclaimDvpPublicInputs,
}

#[derive(Clone, ProofInput)]
pub struct ReclaimDvpPrivateInputs {
    pub tx_context: TxContext,
    pub terms: SwapDvpTerms,
    pub deposit: WalletUtxo,
    pub payment_leg: bool,
    pub refund_destination: ShieldedAddress,
}

#[derive(Clone, PublicInputs)]
pub struct ReclaimDvpPublicInputs {
    pub creator_identity: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_owner: ShieldedAddress,
    pub signer_identity: [u8; 32],
}

impl Circuit for <ReclaimDvp as ProofInput>::Circuit {
    type Output = CheckedTransaction;

    /// 1. Check terms and creator.
    /// 2. Check the signer is the leg's party.
    /// 3. Spend the deposit.
    /// 4. Check DvP PDA ownership.
    /// 5. Check the leg mint.
    /// 6. Create the chosen refund.
    /// 7. Refund the full deposit.
    /// 8. Check the transaction.
    fn circuit(&self) -> Result<Self::Output, CircuitError> {
        let private = &self.private;
        let public = &self.public;

        // 1. Check terms and creator.
        private
            .terms
            .assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check the signer is the leg's party.
        private
            .payment_leg
            .select(
                &private.terms.buyer_identity,
                &private.terms.seller_identity,
            )
            .assert_equal(
                &public.signer_identity,
                "only the leg's party can reclaim it",
            )?;

        // 3. Spend the deposit.
        let asset = private
            .payment_leg
            .select(&private.terms.payment_mint, &private.terms.delivery_mint);
        let mut deposit = TokenUtxos::new_close(core::array::from_ref(&private.deposit))?;

        // 4. Check DvP PDA ownership.
        deposit
            .owner()
            .assert_equal(&public.dvp_owner, "the DvP PDA owns the deposit")?;

        // 5. Check the leg mint.
        deposit
            .asset()
            .assert_equal(&asset, "the deposit belongs to the selected leg")?;

        // 6. Create the chosen refund.
        let mut refund =
            TokenUtxos::new_init(&private.refund_destination, &asset).with_zero_output();

        // 7. Refund the full deposit.
        deposit.transfer_all(&mut refund)?;

        // 8. Check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with_compact_padding()
            .with_token_utxos(deposit)
            .with_token_utxos(refund)
            .check()
    }
}

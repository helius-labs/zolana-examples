use zolana_program::circuit::Utxo;
use zolana_program::{
    circuit::{CheckedTransaction, ConfidentialTransaction, TokenUtxos, UtxoTrait},
    input, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext, ZkProgram,
};

use super::{
    shared::{assert_blinding_seed, close_dvp_state},
    SwapDvpTerms,
};

#[input]
#[derive(ZkProgram)]
pub struct AbortDvp {
    pub private: AbortDvpPrivateInputs,
    pub public: AbortDvpPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct AbortDvpPrivateInputs {
    pub tx_context: TxContext,
    #[client(nested)]
    pub terms: SwapDvpTerms,
    pub dvp_utxo: Utxo,
    pub delivery_deposit: Utxo,
    pub payment_deposit: Utxo,
    pub buyer_rejects: bool,
}

#[input]
#[derive(ClientInputs)]
pub struct AbortDvpPublicInputs {
    pub creator_identity: Field,
    pub dvp_id: Field,
    pub dvp_owner: ShieldedAddress,
    pub signer_identity: Field,
    pub reject: bool,
}

impl AbortDvp {
    /// 1. Check terms and creator.
    /// 2. Check the settlement authority for cancel or seller/buyer for reject.
    /// 3. Check the abort seed and spend the DvP UTXO.
    /// 4. Spend any delivery deposit.
    /// 5. Spend any payment deposit.
    /// 6. Create the seller's refund, including zero.
    /// 7. Create the buyer's refund, including zero.
    /// 8. Refund the delivery deposit to the seller.
    /// 9. Refund the payment deposit to the buyer.
    /// 10. Check the transaction.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;
        let terms = &private.terms;

        // 1. Check terms and creator.
        terms.assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check the settlement authority for cancel or seller/buyer for reject.
        let party = if private.buyer_rejects {
            terms.buyer_identity
        } else {
            terms.seller_identity
        };
        let signer = if public.reject {
            party
        } else {
            terms.settlement_authority_identity
        };
        require_eq(signer, public.signer_identity);

        // 3. Check the abort seed and spend the DvP UTXO.
        assert_blinding_seed(&private.tx_context, crate::ABORT_TAG, &terms.blinding)?;
        let dvp_utxo = close_dvp_state(&private.dvp_utxo, &public.dvp_owner, &public.dvp_id)?;

        // 4. Spend any delivery deposit.
        let mut delivery_deposit =
            TokenUtxos::new_mut(core::array::from_ref(&private.delivery_deposit))?
                .with_optional(&public.dvp_owner, &terms.delivery_mint)
                .close();

        // 5. Spend any payment deposit.
        let mut payment_deposit =
            TokenUtxos::new_mut(core::array::from_ref(&private.payment_deposit))?
                .with_optional(&public.dvp_owner, &terms.payment_mint)
                .close();

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

        // 10. Check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(dvp_utxo)
            .with(delivery_deposit)
            .with(payment_deposit)
            .with(seller_refund)
            .with(buyer_refund)
            .check()
    }
}

impl zolana_program::circuit::PublicInputs for AbortDvpPublicInputs {
    fn hash(&self, transaction_hash: &Field) -> Result<Field, CircuitError> {
        zolana_program::poseidon(&[
            self.creator_identity,
            self.dvp_id,
            self.dvp_owner.owner().hash()?,
            self.signer_identity,
            Field::from(self.reject),
            *transaction_hash,
        ])
    }
}

#[cfg(all(feature = "r1cs-export", not(xark), not(xark_client)))]
impl zolana_program::build::PublicInputLayout for AbortDvpPublicInputs {
    fn layout() -> Vec<(&'static str, zolana_program::build::PublicFieldKind)> {
        use zolana_program::build::PublicFieldKind;
        vec![
            ("creator_identity", PublicFieldKind::Hash),
            ("dvp_id", PublicFieldKind::Hash),
            ("dvp_owner", PublicFieldKind::Hash),
            ("signer_identity", PublicFieldKind::Hash),
            ("reject", PublicFieldKind::Bool),
        ]
    }
}

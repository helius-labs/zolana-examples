use super::{shared::dvp_owner, SwapDvpTerms};
use spr_program::circuit::Utxo;
use spr_program::{
    circuit::{CheckedTransaction, ConfidentialTransaction, PublicInputs, TokenUtxos, UtxoTrait},
    input, require_eq, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext, ZkProgram,
};

#[input]
#[derive(ZkProgram)]
pub struct ReclaimDvp {
    pub private: ReclaimDvpPrivateInputs,
    pub public: ReclaimDvpPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct ReclaimDvpPrivateInputs {
    pub tx_context: TxContext,
    pub terms: SwapDvpTerms,
    pub deposit: Utxo,
    pub payment_leg: bool,
    pub refund_destination: ShieldedAddress,
    pub dvp_address: ShieldedAddress,
}

#[input]
#[derive(PublicInputs)]
pub struct ReclaimDvpPublicInputs {
    pub creator_identity: Field,
    pub dvp_id: Field,
    /// Owner hash of the DvP PDA.
    pub dvp_owner: Field,
    pub signer_identity: Field,
}

impl ReclaimDvp {
    /// 1. Check terms and creator.
    /// 2. Check the signer is the leg's party.
    /// 3. Spend the deposit.
    /// 4. Check DvP PDA ownership.
    /// 5. Check the leg mint.
    /// 6. Create the chosen refund.
    /// 7. Refund the full deposit.
    /// 8. Check the transaction.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;

        // 1. Check terms and creator.
        private
            .terms
            .assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check the signer is the leg's party.
        let signer = if private.payment_leg {
            private.terms.buyer_identity
        } else {
            private.terms.seller_identity
        };
        require_eq(signer, public.signer_identity);

        // 3. Spend the deposit.
        let asset = if private.payment_leg {
            private.terms.payment_mint
        } else {
            private.terms.delivery_mint
        };
        let mut deposit = TokenUtxos::new_mut(core::array::from_ref(&private.deposit))?.close();

        // 4. Check DvP PDA ownership.
        require_eq(
            deposit.owner(),
            *dvp_owner(&private.dvp_address, &public.dvp_owner)?,
        );

        // 5. Check the leg mint.
        require_eq(deposit.asset(), asset);

        // 6. Create the chosen refund.
        let mut refund =
            TokenUtxos::new_init(&private.refund_destination, &asset).with_zero_output();

        // 7. Refund the full deposit.
        deposit.transfer_all(&mut refund)?;

        // 8. Check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(deposit)
            .with(refund)
            .check()
    }
}

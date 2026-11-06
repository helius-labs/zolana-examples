use super::{
    shared::{assert_blinding_seed, dvp_owner},
    SwapDvpTerms,
};
use spr_program::{
    circuit::{CheckedTransaction, ConfidentialTransaction, DataUtxo, PublicInputs},
    input, CircuitError, ClientInputs, Field, ShieldedAddress, TxContext, ZkProgram,
};

#[input]
#[derive(ZkProgram)]
pub struct CreateDvp {
    pub private: CreateDvpPrivateInputs,
    pub public: CreateDvpPublicInputs,
}

#[input]
#[derive(ClientInputs)]
pub struct CreateDvpPrivateInputs {
    pub tx_context: TxContext,
    pub terms: SwapDvpTerms,
    pub dvp_address: ShieldedAddress,
}

#[input]
#[derive(PublicInputs)]
pub struct CreateDvpPublicInputs {
    pub creator_identity: Field,
    pub dvp_id: Field,
    /// Owner hash of the DvP PDA.
    pub dvp_owner: Field,
}

impl CreateDvp {
    /// 1. Check terms and creator.
    /// 2. Check amounts, mints, and time bounds.
    /// 3. Check the create blinding seed.
    /// 4. Create the zero-SOL DvP UTXO.
    /// 5. Reserve the DvP address and check the transaction.
    pub fn execute(&self) -> Result<CheckedTransaction, CircuitError> {
        let private = &self.private;
        let public = &self.public;

        // 1. Check terms and creator.
        private
            .terms
            .assert_dvp(&public.dvp_id, &public.creator_identity)?;

        // 2. Check amounts, mints, and time bounds.
        private.terms.assert_valid_trade()?;

        // 3. Check the create blinding seed.
        assert_blinding_seed(
            &private.tx_context,
            crate::CREATE_TAG,
            &private.terms.blinding,
        )?;

        // 4. Create the zero-SOL DvP UTXO.
        let owner = dvp_owner(&private.dvp_address, &public.dvp_owner)?;
        let dvp_utxo = DataUtxo::new_init_with_hash(owner, &public.dvp_id);

        // 5. Reserve the DvP address and check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(spr_program::circuit::NewAddress::new(
                owner,
                &public.dvp_id,
            )?)
            .with(dvp_utxo)
            .check()
    }
}

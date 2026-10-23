use super::{shared::assert_blinding_seed, SwapDvpTerms};
use zolana_keypair::ShieldedAddress;
use zolana_program::{
    circuit::{CheckedTransaction, Circuit, ConfidentialTransaction, DataUtxo, PublicInputs},
    conversion::ProofInput,
    CircuitError, TxContext, ZkProgramCircuit,
};

#[derive(Clone, ZkProgramCircuit)]
pub struct CreateDvp {
    pub private: CreateDvpPrivateInputs,
    pub public: CreateDvpPublicInputs,
}

#[derive(Clone, ProofInput)]
pub struct CreateDvpPrivateInputs {
    pub tx_context: TxContext,
    pub terms: SwapDvpTerms,
}

#[derive(Clone, PublicInputs)]
pub struct CreateDvpPublicInputs {
    pub creator_identity: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_owner: ShieldedAddress,
}

impl Circuit for <CreateDvp as ProofInput>::Circuit {
    type Output = CheckedTransaction;

    /// 1. Check terms and creator.
    /// 2. Check amounts, mints, and time bounds.
    /// 3. Check the create blinding seed.
    /// 4. Create the zero-SOL DvP UTXO.
    /// 5. Reserve the DvP address and check the transaction.
    fn circuit(&self) -> Result<Self::Output, CircuitError> {
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
        let dvp_utxo = DataUtxo::new_init_with_hash(&public.dvp_owner, &public.dvp_id);

        // 5. Reserve the DvP address and check the transaction.
        ConfidentialTransaction::new(&private.tx_context, public)
            .with(zolana_program::circuit::NewAddress::new(
                &public.dvp_owner,
                &public.dvp_id,
            )?)
            .with(dvp_utxo)
            .check()
    }
}

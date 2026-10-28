use super::{shared::assert_blinding_seed, SwapDvpTerms};
use zolana_program::{
    circuit::{CheckedTransaction, ConfidentialTransaction, DataUtxo},
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
    #[client(nested)]
    pub terms: SwapDvpTerms,
}

#[input]
#[derive(ClientInputs)]
pub struct CreateDvpPublicInputs {
    pub creator_identity: Field,
    pub dvp_id: Field,
    pub dvp_owner: ShieldedAddress,
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

impl zolana_program::circuit::PublicInputs for CreateDvpPublicInputs {
    fn hash(&self, transaction_hash: &Field) -> Result<Field, CircuitError> {
        zolana_program::poseidon(&[
            self.creator_identity,
            self.dvp_id,
            self.dvp_owner.owner().hash()?,
            *transaction_hash,
        ])
    }
}

#[cfg(all(feature = "r1cs-export", not(xark), not(xark_client)))]
impl zolana_program::build::PublicInputLayout for CreateDvpPublicInputs {
    fn layout() -> Vec<(&'static str, zolana_program::build::PublicFieldKind)> {
        use zolana_program::build::PublicFieldKind;
        vec![
            ("creator_identity", PublicFieldKind::Hash),
            ("dvp_id", PublicFieldKind::Hash),
            ("dvp_owner", PublicFieldKind::Hash),
        ]
    }
}

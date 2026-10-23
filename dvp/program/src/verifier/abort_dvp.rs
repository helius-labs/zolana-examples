use zolana_program::{
    transaction_hash,
    verifier::{groth16_solana, public_hash, verify_groth16, Field, VerifyError},
    CompressedProof, PublicTransfer,
};

include!("abort_dvp.vk.rs");

pub struct PublicInputs {
    pub creator_identity: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_owner: [u8; 32],
    pub signer_identity: [u8; 32],
    pub reject: bool,
}

impl PublicInputs {
    pub fn hash(&self, transaction_hash: &[u8; 32]) -> Result<[u8; 32], VerifyError> {
        public_hash(
            &[
                Field::Hash(&self.creator_identity),
                Field::Hash(&self.dvp_id),
                Field::Hash(&self.dvp_owner),
                Field::Hash(&self.signer_identity),
                Field::Bool(self.reject),
            ],
            transaction_hash,
        )
    }
}

pub struct Verifier<'a> {
    pub proof: &'a CompressedProof,
    pub public_inputs: PublicInputs,
    pub private_tx_hash: &'a [u8; 32],
    pub transfers: &'a [PublicTransfer],
}

impl Verifier<'_> {
    pub fn verify(&self) -> Result<(), VerifyError> {
        let transaction_hash = transaction_hash(self.private_tx_hash, self.transfers)
            .map_err(|_| VerifyError::PublicInputs)?;
        verify_groth16(
            self.proof,
            self.public_inputs.hash(&transaction_hash)?,
            &VERIFYINGKEY,
        )
    }
}

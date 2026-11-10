use zolana_program::{
    verifier::{groth16_solana, public_hash, verify_groth16, Field, VerifyError},
    CompressedProof,
};

include!("reclaim_dvp.vk.rs");

pub struct PublicInputs {
    pub creator_identity: [u8; 32],
    pub dvp_id: [u8; 32],
    pub dvp_owner: [u8; 32],
    pub signer_identity: [u8; 32],
}

impl PublicInputs {
    pub fn hash(&self, private_tx_hash: &[u8; 32]) -> Result<[u8; 32], VerifyError> {
        public_hash(
            &[
                Field::Hash(&self.creator_identity),
                Field::Hash(&self.dvp_id),
                Field::Hash(&self.dvp_owner),
                Field::Hash(&self.signer_identity),
            ],
            private_tx_hash,
        )
    }
}

pub fn verify(
    proof: &CompressedProof,
    public_inputs: &PublicInputs,
    private_tx_hash: &[u8; 32],
) -> Result<(), VerifyError> {
    verify_groth16(proof, public_inputs.hash(private_tx_hash)?, &VERIFYINGKEY)
}

use dvp_program::instructions::create_dvp::CreateDvpProof;
use zolana_gnark_ffi_prover::CompressedProof;

#[derive(Debug, Clone, Copy)]
pub struct DvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

impl From<DvpProof> for CreateDvpProof {
    fn from(proof: DvpProof) -> Self {
        Self {
            proof_a: proof.proof_a,
            proof_b: proof.proof_b,
            proof_c: proof.proof_c,
        }
    }
}

/// Every DvP circuit is registered without a BSB22 commitment, so the
/// compressed proof carries none.
impl From<CompressedProof> for DvpProof {
    fn from(proof: CompressedProof) -> Self {
        Self {
            proof_a: proof.proof_a,
            proof_b: proof.proof_b,
            proof_c: proof.proof_c,
        }
    }
}

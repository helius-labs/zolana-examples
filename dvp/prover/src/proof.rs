use dvp_program::instructions::{
    abort_dvp::AbortDvpProof, create_dvp::CreateDvpProof, reclaim_dvp::ReclaimDvpProof,
    settle_dvp::SettleDvpProof,
};
use zolana_gnark_ffi_prover::CompressedProof;

use crate::CircuitId;

#[derive(Debug, thiserror::Error)]
pub enum ProveError {
    #[error(transparent)]
    Prover(#[from] zolana_gnark_ffi_prover::Error),
    /// The program verifies every DvP proof without a BSB22 commitment, so a
    /// proof that carries one cannot verify.
    #[error("the {0:?} proof carries a BSB22 commitment")]
    UnexpectedCommitment(CircuitId),
}

#[derive(Debug, Clone, Copy)]
pub struct DvpProof {
    pub proof_a: [u8; 32],
    pub proof_b: [u8; 64],
    pub proof_c: [u8; 32],
}

impl DvpProof {
    /// Every DvP circuit is registered without a BSB22 commitment; a proof
    /// of `circuit` with one is refused rather than stripped of it.
    fn new(circuit: CircuitId, proof: CompressedProof) -> Result<Self, ProveError> {
        if proof.commitment.is_some() {
            return Err(ProveError::UnexpectedCommitment(circuit));
        }
        Ok(Self {
            proof_a: proof.proof_a,
            proof_b: proof.proof_b,
            proof_c: proof.proof_c,
        })
    }

    /// Proves `circuit` on `witness` with the shared prover.
    pub(crate) fn prove(
        circuit: CircuitId,
        witness: &zolana_gnark_ffi_prover::ProofInputMap,
    ) -> Result<Self, ProveError> {
        let proof = crate::PROVER.prove(circuit, witness)?.compress()?;
        Self::new(circuit, proof)
    }
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

impl From<DvpProof> for ReclaimDvpProof {
    fn from(proof: DvpProof) -> Self {
        Self {
            proof_a: proof.proof_a,
            proof_b: proof.proof_b,
            proof_c: proof.proof_c,
        }
    }
}

impl From<DvpProof> for SettleDvpProof {
    fn from(proof: DvpProof) -> Self {
        Self {
            proof_a: proof.proof_a,
            proof_b: proof.proof_b,
            proof_c: proof.proof_c,
        }
    }
}

impl From<DvpProof> for AbortDvpProof {
    fn from(proof: DvpProof) -> Self {
        Self {
            proof_a: proof.proof_a,
            proof_b: proof.proof_b,
            proof_c: proof.proof_c,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_with_a_commitment_is_refused() {
        let proof = CompressedProof {
            proof_a: [1; 32],
            proof_b: [2; 64],
            proof_c: [3; 32],
            commitment: Some(([4; 32], [5; 32])),
        };
        let error = DvpProof::new(CircuitId::SettleDvp, proof)
            .expect_err("a commitment the program does not verify");
        assert!(matches!(
            error,
            ProveError::UnexpectedCommitment(CircuitId::SettleDvp)
        ));
    }
}

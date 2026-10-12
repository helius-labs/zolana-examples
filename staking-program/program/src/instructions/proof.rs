use groth16_solana::{
    decompression::{decompress_g1, decompress_g2},
    groth16::{Groth16Verifier, Groth16Verifyingkey},
};
use pinocchio::ProgramResult;
use wincode::{SchemaRead, SchemaWrite};

use crate::error::StakingError;

/// AES lookup arguments use one BSB22 commitment and its proof of knowledge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct LogicProof {
    pub a: [u8; 32],
    pub b: [u8; 64],
    pub c: [u8; 32],
    pub commitment: [u8; 32],
    pub commitment_pok: [u8; 32],
}

pub fn verify(
    proof: &LogicProof,
    public_input: [u8; 32],
    key: &Groth16Verifyingkey,
) -> ProgramResult {
    let error = StakingError::ProofVerificationFailed;
    let a = decompress_g1(&proof.a).map_err(|_| error)?;
    let b = decompress_g2(&proof.b).map_err(|_| error)?;
    let c = decompress_g1(&proof.c).map_err(|_| error)?;
    let commitment = decompress_g1(&proof.commitment).map_err(|_| error)?;
    let pok = decompress_g1(&proof.commitment_pok).map_err(|_| error)?;
    let public_inputs = [public_input];
    Groth16Verifier::new_with_commitment(&a, &b, &c, &commitment, &pok, &public_inputs, key)
        .map_err(|_| error)?
        .verify()
        .map_err(|_| error.into())
}

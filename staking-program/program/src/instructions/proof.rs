use groth16_solana::{
    decompression::{decompress_g1, decompress_g2},
    groth16::{Groth16Verifier, Groth16Verifyingkey},
};
use pinocchio::ProgramResult;
use wincode::{SchemaRead, SchemaWrite};

use crate::error::StakingError;

pub fn ciphertext_context_hash<const LEN: usize>(
    data: &[u8],
    pk: &[u8; 33],
    salt: &[u8; 16],
) -> Result<[u8; 32], pinocchio::error::ProgramError> {
    let pk_hash =
        zolana_hasher::primitives::hash_bytes(pk).map_err(|_| StakingError::HashingFailed)?;
    ciphertext_context_hash_with_pk_hash::<LEN>(data, &pk_hash, salt)
}

/// [`ciphertext_context_hash`] with `hash_bytes(pk)` computed once per batch.
pub fn ciphertext_context_hash_with_pk_hash<const LEN: usize>(
    data: &[u8],
    pk_hash: &[u8; 32],
    salt: &[u8; 16],
) -> Result<[u8; 32], pinocchio::error::ProgramError> {
    use zolana_hasher::{
        primitives::{hash_bytes, right_align},
        Hasher, Poseidon,
    };
    let ciphertext: &[u8; LEN] = data
        .try_into()
        .map_err(|_| StakingError::InvalidCiphertext)?;
    if ciphertext.first() != Some(&1) {
        return Err(StakingError::InvalidCiphertext.into());
    }
    Poseidon::hashv(&[
        &hash_bytes(ciphertext).map_err(|_| StakingError::HashingFailed)?,
        pk_hash,
        &right_align(salt),
    ])
    .map_err(|_| StakingError::HashingFailed.into())
}

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

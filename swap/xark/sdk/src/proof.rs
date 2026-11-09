//! Loads checksum-pinned Xark artifacts and converts proofs to the Solana ABI.
use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use spr_program::{
    proof::{compressed_proof, SolanaVerifyingKey},
    CompressedProof, ProgramProofInputs,
};
use xark_groth16::PkSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitId {
    Make,
    Take,
    Cancel,
}

impl CircuitId {
    pub const ALL: [Self; 3] = [Self::Make, Self::Take, Self::Cancel];

    /// The circuit's directory under [`keys_dir`]: `zolana zk compile` names
    /// it after the snake-cased program struct.
    pub fn name(self) -> &'static str {
        match self {
            Self::Make => "make",
            Self::Take => "take",
            Self::Cancel => "cancel",
        }
    }
}

/// `SWAP_KEYS_DIR`, else `target/zk/swap-program`, where
/// `zolana zk compile --test` writes the artifacts.
pub fn keys_dir() -> PathBuf {
    std::env::var_os("SWAP_KEYS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/zk/swap-program")
        })
}

/// The manifest is committed source metadata, never downloaded with the
/// artifacts.
fn artifact_checksum(id: CircuitId, artifact: &str) -> Result<[u8; 32]> {
    let filename = format!("{}/{artifact}", id.name());
    let digest = include_str!("../../swap-keys.CHECKSUM")
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            (parts.next()? == filename).then_some(hash)
        })
        .with_context(|| format!("no checksum for {filename}; regenerate the swap artifacts"))?;
    ensure!(digest.len() == 64, "invalid swap artifact checksum");
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(digest.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(bytes)
}

fn checked_artifact(id: CircuitId, artifact: &str) -> Result<Vec<u8>> {
    let path = keys_dir().join(id.name()).join(artifact);
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "reading {}: run `zolana zk compile --test` to build the swap artifacts",
            path.display()
        )
    })?;
    ensure!(
        <[u8; 32]>::from(Sha256::digest(&bytes)) == artifact_checksum(id, artifact)?,
        "swap artifact checksum mismatch: {}",
        path.display()
    );
    Ok(bytes)
}

/// Loads the circuit from a private copy of its checked bytecode and proving
/// key. Xark reads both from its directory again while proving, and rebuilds
/// a circuit whose recorded sources look newer; the copy has no `deps` and
/// lives outside `target/xark`, so it proves exactly the checked bytes. Keep
/// the directory alive while the circuit is in use.
pub fn checked_circuit(id: CircuitId) -> Result<(tempfile::TempDir, xark::prover::Circuit)> {
    let private = tempfile::Builder::new().prefix("swap-circuit-").tempdir()?;
    let dir = private.path().join(id.name());
    std::fs::create_dir(&dir)?;
    for artifact in ["circuit.xbc", "pk.bin"] {
        std::fs::write(dir.join(artifact), checked_artifact(id, artifact)?)?;
    }
    // Source lines only label failing constraints in error messages.
    if let Ok(sources) = std::fs::read(keys_dir().join(id.name()).join("sources.json")) {
        std::fs::write(dir.join("sources.json"), sources)?;
    }
    let circuit = xark::prover::circuit_at(&dir);
    Ok((private, circuit))
}

/// Proves `inputs` with the checked circuit and checks the proof against the
/// checked verifying key before returning it.
pub fn prove(id: CircuitId, inputs: &ProgramProofInputs) -> Result<CompressedProof> {
    let key = verifying_key(id)?;
    let (_private, circuit) = checked_circuit(id)?;
    let proven = circuit.prove(inputs.clone())?;
    let proof = compressed_proof(&proven.proof).map_err(anyhow::Error::msg)?;
    let encoded = inputs.encoded();
    let hash = encoded.get("public_hash").context("missing public hash")?;
    let hash = spr_program::from_decimal(hash)?;
    spr_program::verifier::verify_groth16(&proof, spr_program::to_bytes(&hash), &key.as_key())
        .map_err(|error| anyhow::anyhow!("invalid swap proof: {error:?}"))?;
    Ok(proof)
}

pub fn verifying_key(id: CircuitId) -> Result<SolanaVerifyingKey> {
    SolanaVerifyingKey::from_xark(&checked_artifact(id, "vk.bin")?).map_err(anyhow::Error::msg)
}

/// The constraint count of a circuit's checked artifacts.
pub fn constraint_count(id: CircuitId) -> Result<usize> {
    let pk = checked_artifact(id, "pk.bin")?;
    let key = xark_groth16::SerializedPk::from_bytes(&pk)?;
    let rows = xark::prover::rows_for_key(
        &checked_artifact(id, "circuit.xbc")?,
        key.header().a_len,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    let count = rows.num_constraints();
    ensure!(count != 0, "empty swap circuit");
    Ok(count)
}

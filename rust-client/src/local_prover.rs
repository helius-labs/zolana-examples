//! A [`Prover`] that proves on this machine, with gnark in process, instead of
//! sending the request to a prover server.
//!
//! The request carries the nullifier secrets of the notes it spends, so proving
//! locally keeps them on this machine.

use std::{fs::File, io, path::PathBuf, sync::Mutex};

use sha2::{Digest, Sha256};
use zolana_client::{
    prover::{ExpectedProvingKey, ProveRequest},
    ClientError, Proof, Prover,
};

/// Proves with the proving keys in `keys_dir`, keeping the last one loaded.
pub struct LocalProver {
    keys_dir: PathBuf,
    loaded: Mutex<Option<(String, rust_gnark::PreparedProver)>>,
}

impl LocalProver {
    /// `keys_dir` holds `.key` files named as in the proving-key lockfile, for
    /// example `transfer_confidential_1_2.key`. The zolana prover server keeps
    /// its verified downloads in `~/.config/zolana/proving-keys`.
    pub fn new(keys_dir: impl Into<PathBuf>) -> Self {
        Self {
            keys_dir: keys_dir.into(),
            loaded: Mutex::new(None),
        }
    }

    fn prove_json(&self, body: &str, key: &ExpectedProvingKey) -> Result<String, String> {
        let mut loaded = self.loaded.lock().map_err(|_| "prover poisoned")?;
        if loaded.as_ref().map(|(name, _)| name) != Some(&key.name) {
            // gnark holds one prepared key: drop the previous one first.
            *loaded = None;
            *loaded = Some((key.name.clone(), self.load(key)?));
        }
        let (_, prover) = loaded.as_ref().ok_or("no proving key loaded")?;
        let proof = prover
            .prove_request(body)
            .map_err(|_| "proof failed".to_string())?;
        Ok(proof.proof_json)
    }

    /// Load `key` after checking the file against the sha256 the on-chain
    /// verifying key pins. A proof from any other key fails on-chain.
    fn load(&self, key: &ExpectedProvingKey) -> Result<rust_gnark::PreparedProver, String> {
        let path = self.keys_dir.join(&key.name);
        let read_error =
            |error: io::Error| format!("could not read proving key {}: {error}", path.display());
        let mut hasher = Sha256::new();
        io::copy(&mut File::open(&path).map_err(read_error)?, &mut hasher).map_err(read_error)?;
        if hasher.finalize().as_slice() != key.sha256 {
            return Err(format!(
                "proving key {} does not match its verifying key",
                path.display()
            ));
        }
        let path = path.to_str().ok_or("proving key path is not UTF-8")?;
        rust_gnark::PreparedProver::load_key(path)
            .map_err(|_| format!("could not load proving key {}", key.name))
    }
}

impl Prover for LocalProver {
    fn prove(&self, request: &dyn ProveRequest) -> Result<Proof, ClientError> {
        let proof_json = self
            .prove_json(&request.body()?, &request.proving_key()?)
            .map_err(ClientError::Prover)?;
        Proof::from_gnark_json(&proof_json)
    }
}

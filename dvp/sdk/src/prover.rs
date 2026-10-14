use anyhow::Result;
use dvp_prover::{CreateDvpProofInputs, DvpProof};

use crate::err;

#[derive(Default)]
pub struct DvpProverClient;

impl DvpProverClient {
    pub fn new() -> Self {
        Self
    }

    pub fn prove_create_dvp(&self, inputs: &CreateDvpProofInputs) -> Result<DvpProof> {
        inputs.prove().map_err(err)
    }
}

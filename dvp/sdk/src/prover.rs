use anyhow::Result;
use dvp_prover::{CreateDvpProofInputs, DvpProof, ReclaimDvpProofInputs, SettleDvpProofInputs};

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

    pub fn prove_reclaim_dvp(&self, inputs: &ReclaimDvpProofInputs) -> Result<DvpProof> {
        inputs.prove().map_err(err)
    }

    pub fn prove_settle_dvp(&self, inputs: &SettleDvpProofInputs) -> Result<DvpProof> {
        inputs.prove().map_err(err)
    }
}

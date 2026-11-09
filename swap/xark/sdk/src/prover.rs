use anyhow::Result;
use swap_program::circuits::{Cancel, Make, Take};

use crate::{
    proof::{prove, CircuitId},
    CancelProof, MakeProof, TakeProof,
};

/// Proves the swap circuits in process from the checksum-pinned Xark
/// artifacts. One `prove_*` method per circuit, no data processing.
#[derive(Default)]
pub struct SwapProverClient;

impl SwapProverClient {
    pub fn new() -> Self {
        Self
    }

    pub fn prove_make(&self, inputs: &Make) -> Result<MakeProof> {
        prove(CircuitId::Make, inputs, || inputs.execute())
    }

    pub fn prove_take(&self, inputs: &Take) -> Result<TakeProof> {
        prove(CircuitId::Take, inputs, || inputs.execute())
    }

    pub fn prove_cancel(&self, inputs: &Cancel) -> Result<CancelProof> {
        prove(CircuitId::Cancel, inputs, || inputs.execute())
    }
}

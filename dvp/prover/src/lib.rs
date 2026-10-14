//! Proof inputs and the gnark prover for the DvP circuits.
pub mod create_dvp;
pub mod proof;
pub mod reclaim_dvp;
pub mod swap_dvp;

pub use create_dvp::CreateDvpProofInputs;
pub use proof::DvpProof;
pub use reclaim_dvp::ReclaimDvpProofInputs;
pub use swap_dvp::{DestinationProofInput, SwapDvpProofInput};
pub use zolana_client::ProofInputUtxo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitId {
    CreateDvp,
    ReclaimDvp,
}

impl zolana_gnark_ffi_prover::Circuit for CircuitId {
    const ALL: &'static [Self] = &[Self::CreateDvp, Self::ReclaimDvp];

    fn name(self) -> &'static str {
        match self {
            Self::CreateDvp => "create_dvp",
            Self::ReclaimDvp => "reclaim_dvp",
        }
    }
}

pub static PROVER: zolana_gnark_ffi_prover::Prover<CircuitId> =
    zolana_gnark_ffi_prover::prover!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/gnark"));

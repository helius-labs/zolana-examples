//! Proof inputs and the gnark prover for the DvP circuits.
pub mod abort_dvp;
pub mod create_dvp;
pub mod proof;
pub mod reclaim_dvp;
pub mod settle_dvp;
pub mod swap_dvp;

pub use abort_dvp::{AbortDvpProofInputs, AbortMode};
pub use create_dvp::CreateDvpProofInputs;
pub use proof::{DvpProof, ProveError};
pub use reclaim_dvp::ReclaimDvpProofInputs;
pub use settle_dvp::SettleDvpProofInputs;
pub use swap_dvp::{DestinationProofInput, SwapDvpProofInput};
pub use zolana_client::ProofInputUtxo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitId {
    CreateDvp,
    ReclaimDvp,
    SettleDvp,
    AbortDvp,
}

impl zolana_gnark_ffi_prover::Circuit for CircuitId {
    const ALL: &'static [Self] = &[
        Self::CreateDvp,
        Self::ReclaimDvp,
        Self::SettleDvp,
        Self::AbortDvp,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::CreateDvp => "create_dvp",
            Self::ReclaimDvp => "reclaim_dvp",
            Self::SettleDvp => "settle_dvp",
            Self::AbortDvp => "abort_dvp",
        }
    }
}

pub static PROVER: zolana_gnark_ffi_prover::Prover<CircuitId> =
    zolana_gnark_ffi_prover::prover!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/gnark"));

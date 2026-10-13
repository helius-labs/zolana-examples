pub use zolana_gnark_ffi_prover::{CompressedProof, ProofInputMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitId {
    Intent,
    Cancel,
    Settlement,
}

impl zolana_gnark_ffi_prover::Circuit for CircuitId {
    const ALL: &'static [Self] = &[Self::Intent, Self::Cancel, Self::Settlement];

    fn name(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Cancel => "cancel",
            Self::Settlement => "settlement",
        }
    }
}

pub static PROVER: zolana_gnark_ffi_prover::Prover<CircuitId> =
    zolana_gnark_ffi_prover::prover!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/gnark"));

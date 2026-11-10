pub mod cancel;
pub mod make;
pub mod order_terms;
pub mod proof;
pub mod take;

pub use cancel::CancelProofInputs;
pub use make::MakeProofInputs;
pub use order_terms::OrderTermsProofInput;
pub use proof::OrderProof;
pub use spr_client::ProofInputUtxo;
pub use take::TakeProofInputs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitId {
    Make,
    Cancel,
    Take,
}

impl spr_gnark_ffi_prover::Circuit for CircuitId {
    const ALL: &'static [Self] = &[Self::Make, Self::Cancel, Self::Take];

    fn name(self) -> &'static str {
        match self {
            Self::Make => "make",
            Self::Cancel => "cancel",
            Self::Take => "take",
        }
    }
}

pub static PROVER: spr_gnark_ffi_prover::Prover<CircuitId> =
    spr_gnark_ffi_prover::prover!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/gnark"));

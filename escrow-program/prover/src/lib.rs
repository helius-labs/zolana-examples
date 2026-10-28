pub mod escrow;
pub mod escrow_terms;
pub mod proof;
pub mod withdraw;

pub use escrow::EscrowProofInputs;
pub use escrow_terms::EscrowTermsProofInput;
pub use proof::TimelockProof;
pub use spr_client::ProofInputUtxo;
pub use withdraw::WithdrawProofInputs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitId {
    Escrow,
    Withdraw,
}

impl spr_gnark_ffi_prover::Circuit for CircuitId {
    const ALL: &'static [Self] = &[Self::Escrow, Self::Withdraw];

    fn name(self) -> &'static str {
        match self {
            Self::Escrow => "escrow",
            Self::Withdraw => "withdraw",
        }
    }
}

pub static PROVER: spr_gnark_ffi_prover::Prover<CircuitId> =
    spr_gnark_ffi_prover::prover!(concat!(env!("CARGO_MANIFEST_DIR"), "/../build/gnark"));

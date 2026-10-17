//! Client SDK for the DvP program: terms, proofs, instruction builders and discovery.
pub mod encryption;
pub mod index;
pub mod instructions;
pub mod proof;
mod shared;
pub mod state;

/// The program's wire types, which the instruction builders' fields name.
pub use dvp_program::{
    instructions::{
        abort_dvp::AbortDvpProof,
        cancel_dvp::CancelDvpIxData,
        create_dvp::{CreateDvpIxData, CreateDvpProof},
        reclaim_dvp::{ReclaimDvpIxData, ReclaimDvpProof},
        reject_dvp::RejectDvpIxData,
        settle_dvp::{SettleDvpIxData, SettleDvpProof},
    },
    tag,
};

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

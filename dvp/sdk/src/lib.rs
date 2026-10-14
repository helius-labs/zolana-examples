//! Client SDK for the DvP program: terms, proofs, instruction builders and discovery.
pub mod encryption;
pub mod index;
pub mod instructions;
pub mod prover;
mod shared;
pub mod state;

pub use dvp_program::{
    instructions::{
        create_dvp::{CreateDvpIxData, CreateDvpProof},
        reclaim_dvp::{ReclaimDvpIxData, ReclaimDvpProof},
        settle_dvp::{SettleDvpIxData, SettleDvpProof},
    },
    tag, DVP_AUTHORITY_PDA_SEED,
};

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

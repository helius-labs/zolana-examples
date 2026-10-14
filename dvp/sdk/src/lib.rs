//! Client SDK for the DvP program: terms, proofs, instruction builders and discovery.
pub mod state;

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

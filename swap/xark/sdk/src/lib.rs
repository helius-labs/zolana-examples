//! Client SDK for the swap program: order state, transaction builders, Xark
//! proofs, instruction builders and order discovery.
pub mod index;
pub mod instructions;
pub mod proof;
pub mod shared;
pub mod state;

use solana_address::Address;
use spr_program::ProgramOwner;
/// The program's wire types, which the instruction builders' fields name.
pub use swap_program::{
    instructions::{
        cancel::{CancelIxData, CancelProof},
        make::{MakeIxData, MakeProof, MarkerData},
        take::{TakeIxData, TakeProof},
    },
    order_authority_seeds, tag, ORDER_AUTHORITY_PDA_SEED,
};

/// The order-authority PDA of `maker`, `[ORDER_AUTHORITY_PDA_SEED, maker]`,
/// which the swap program signs with (`invoke_signed`) to spend the maker's
/// order UTXOs. It owns them, holds no data, and is never created.
pub fn order_authority_pda(maker: &Address) -> Address {
    *order_authority(maker).pda()
}

/// The order-authority PDA of `maker` as a UTXO owner, under the zero
/// nullifier key. Its view tag finds the maker's order UTXOs.
pub fn order_authority(maker: &Address) -> ProgramOwner {
    ProgramOwner::find(&order_authority_seeds(maker.as_array()), &swap_program::ID)
}

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

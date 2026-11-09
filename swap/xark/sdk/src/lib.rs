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
    tag, ORDER_AUTHORITY_PDA_SEED,
};

/// The order-authority PDA the swap program signs with (`invoke_signed`) to
/// spend an order UTXO. It owns the order UTXO, holds no data, and is never
/// created.
pub fn order_authority_pda() -> Address {
    let (pda, _bump) =
        Address::find_program_address(&[ORDER_AUTHORITY_PDA_SEED], &swap_program::ID);
    pda
}

/// The order-authority PDA as a UTXO owner, under the zero nullifier key.
pub fn order_authority() -> ProgramOwner {
    ProgramOwner::new(order_authority_pda())
}

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

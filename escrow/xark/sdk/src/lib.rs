//! Client SDK for the timelock escrow program: escrow terms, transaction
//! builders, Xark proofs and instruction builders.
pub mod instructions;
pub mod proof;
mod shared;
pub mod state;

use solana_address::Address;
use spr_program::ProgramOwner;
/// The program's wire types, which the instruction builders' fields name.
pub use timelock_escrow_program::{
    escrow_authority_seeds,
    instructions::{
        escrow::{EscrowIxData, EscrowProof},
        withdraw::{WithdrawIxData, WithdrawProof},
    },
    tag, ESCROW_AUTHORITY_PDA_SEED,
};

/// `creator`'s escrow-authority PDA (seeds `[b"escrow_authority", creator]`),
/// which the timelock escrow program signs with (`invoke_signed`) to spend
/// `creator`'s escrow UTXOs. It owns them, holds no data, and is never
/// created.
pub fn escrow_authority_pda(creator: &Address) -> Address {
    *escrow_authority(creator).pda()
}

/// `creator`'s escrow-authority PDA as a UTXO owner, under the zero nullifier
/// key. Its confidential view tag finds `creator`'s escrow UTXOs.
pub fn escrow_authority(creator: &Address) -> ProgramOwner {
    ProgramOwner::find(
        &escrow_authority_seeds(creator),
        &timelock_escrow_program::ID,
    )
}

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

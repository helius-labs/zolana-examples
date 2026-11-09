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
    instructions::{
        escrow::{EscrowIxData, EscrowProof},
        withdraw::{WithdrawIxData, WithdrawProof},
    },
    tag, ESCROW_AUTHORITY_PDA_SEED,
};

/// The escrow-authority PDA the timelock escrow program signs with
/// (`invoke_signed`) to spend an escrow UTXO. It owns the escrow UTXO, holds
/// no data, and is never created.
pub fn escrow_authority_pda() -> Address {
    let (pda, _bump) =
        Address::find_program_address(&[ESCROW_AUTHORITY_PDA_SEED], &timelock_escrow_program::ID);
    pda
}

/// The escrow-authority PDA as a UTXO owner, under the zero nullifier key.
pub fn escrow_authority() -> ProgramOwner {
    ProgramOwner::new(escrow_authority_pda())
}

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

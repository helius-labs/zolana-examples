pub mod instructions;
pub mod prover;
pub mod shared;
pub mod state;

use solana_pubkey::Pubkey;
pub use timelock_escrow_program::{
    instructions::{
        escrow::{EscrowIxData, EscrowProof},
        withdraw::{WithdrawIxData, WithdrawProof},
    },
    tag, ESCROW_AUTHORITY_PDA_SEED,
};

/// The creator's escrow-authority PDA (seeds `[ESCROW_AUTHORITY_PDA_SEED,
/// creator]`) the timelock escrow program signs with (`invoke_signed`) to
/// spend the creator's escrow UTXOs. It owns them
/// (`PublicKey::from_ed25519(pda)`, nullifier secret 0), holds no data, and is
/// never created. A creator finds its escrow UTXOs by deriving it.
pub fn escrow_authority_pda(creator: &Pubkey) -> Pubkey {
    let (pda, _bump) = Pubkey::find_program_address(
        &timelock_escrow_program::escrow_authority_seeds(creator),
        &timelock_escrow_program::ID,
    );
    pda
}

pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

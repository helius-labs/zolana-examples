pub mod index;
pub mod instructions;
pub mod prover;
pub mod shared;
pub mod state;

use solana_pubkey::Pubkey;
pub use swap_program::{
    instructions::{
        cancel::{CancelIxData, CancelProof},
        make::{MakeIxData, MakeProof, MarkerData},
        take::{TakeIxData, TakeProof},
    },
    tag, ORDER_AUTHORITY_PDA_SEED,
};

/// The maker's order-authority PDA, seeds `[ORDER_AUTHORITY_PDA_SEED, maker]`.
/// The swap program signs with it (`invoke_signed`) to spend the maker's order
/// UTXOs. It owns every order UTXO of that maker
/// (`PublicKey::from_ed25519(pda)`), so a maker finds its program UTXOs by
/// deriving it. It holds no data and is never created.
pub fn order_authority_pda(maker: &Pubkey) -> Pubkey {
    let (pda, _bump) = Pubkey::find_program_address(
        &swap_program::order_authority_seeds(maker),
        &swap_program::ID,
    );
    pda
}
pub(crate) fn err(e: impl core::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

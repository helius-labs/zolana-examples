use anyhow::Result;
use solana_address::Address;
use spr_keypair::ShieldedKeypair;
use spr_program::DataUtxo;
use spr_transaction::{AssetRegistry, ShieldedTransaction, SppProofOutputUtxo};
/// The escrow terms, the escrow UTXO's state: the creator and the unlock
/// timestamp. The SDK hashes them into the escrow UTXO's data hash and
/// encrypts them as its data.
pub use timelock_escrow_program::circuits::EscrowTerms;

use crate::{err, escrow_authority};

/// The escrow UTXO with its decoded terms: `amount` of `asset` owned by the
/// creator's escrow-authority PDA under the zero nullifier key. Its opening is the
/// complete spend capability; the program signs for the PDA only through
/// `withdraw`.
pub type EscrowUtxo = DataUtxo<EscrowTerms>;

/// The creator's open escrows in `transactions`: the outputs of the
/// creator's escrow-authority PDA encrypted to the creator that decode as
/// escrow terms and that the same transactions do not spend. Fetch
/// `transactions` by that PDA's confidential view tag
/// ([`escrow_authority`]`(creator).confidential_view_tag()`).
pub fn open_escrows(
    creator: &ShieldedKeypair,
    transactions: &[ShieldedTransaction],
    assets: &AssetRegistry,
) -> Result<Vec<EscrowUtxo>> {
    let creator_address = creator
        .shielded_address()
        .map_err(err)?
        .solana_address()
        .map_err(err)?;
    escrow_authority(&creator_address)
        .decrypt_data_utxos::<EscrowTerms>(&creator.viewing_key, transactions, assets)
        .map_err(err)
}

/// The escrow UTXO `creator`'s escrow transaction outputs as `output`, once
/// appended at `leaf_index` of tree `tree_id`, with the terms decoded from its
/// data. Fails unless `creator`'s escrow-authority PDA owns `output`.
pub fn escrow_utxo(
    creator: &Address,
    output: &SppProofOutputUtxo,
    tree_id: u16,
    leaf_index: u64,
) -> Result<EscrowUtxo> {
    let input = escrow_authority(creator)
        .input(output, tree_id, leaf_index)
        .map_err(err)?;
    EscrowUtxo::try_from(input).map_err(err)
}

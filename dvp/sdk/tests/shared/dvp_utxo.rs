//! The DvP address and DvP UTXO blinding, derived without the sdk, for the
//! circuit tests that create or spend the DvP UTXO.

use dvp_program::{instructions::shared::u64_right_align, CREATE_TAG, DVP_ADDRESS_TREE_ID};
use dvp_prover::ProofInputUtxo;
use dvp_sdk::state::SwapDvp;
use zolana_keypair::hash::poseidon;
use zolana_transaction::utxo::{derive_output_blinding_seed, derive_transact_output_blinding};

/// The address an address slot owned by `owner_hash` with seed `dvp_id`
/// creates in tree `tree_id`.
pub fn address(owner_hash: &[u8; 32], dvp_id: &[u8; 32], tree_id: u16) -> [u8; 32] {
    let slot = ProofInputUtxo {
        domain: u64_right_align(2),
        tree_id: u64_right_align(u64::from(tree_id)),
        owner_hash: *owner_hash,
        blinding: *dvp_id,
        ..Default::default()
    };
    poseidon(&[&slot.hash().expect("slot hash"), dvp_id, &[0u8; 32]]).expect("address")
}

/// The DvP UTXO's blinding: output 0 of the create transaction, whose first
/// nullifier is the address the address slot creates in the pinned tree.
pub fn dvp_utxo_blinding(terms: &SwapDvp, owner_hash: &[u8; 32], dvp_id: &[u8; 32]) -> [u8; 32] {
    let address = address(owner_hash, dvp_id, DVP_ADDRESS_TREE_ID);
    let create_seed =
        poseidon(&[&u64_right_align(u64::from(CREATE_TAG)), &terms.blinding]).expect("create seed");
    let seed = derive_output_blinding_seed(&address, &create_seed).expect("create output seed");
    derive_transact_output_blinding(&address, &seed, 0).expect("dvp utxo blinding")
}

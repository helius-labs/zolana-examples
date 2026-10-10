//! The order address: the on-chain record that an order was filled.
//!
//! The market maker's fill transfer spends one more input slot than its UTXOs
//! need: an SPP address slot whose nullifier is the order address. SPP creates
//! the nullifier PDA of every nullifier a `transact` publishes and rejects a
//! transaction whose nullifier PDA already exists with
//! `ShieldedPoolError::NullifierAlreadyQueued` (custom code 7043) before the
//! proof is verified, so a second transaction filling the same order fails at
//! preflight. Once a forester inserts the nullifier into the tree and closes
//! the PDA, the address fails the slot's non-inclusion proof instead, so the
//! order can never be filled twice.
//!
//! The slot costs the market maker no rent: the nullifier PDA's rent is paid
//! by the tree account, and the fee payer pays only the forester fee of one
//! more input. It adds no signer either: the slot's owner is the fee payer's
//! signing key, which already signs as fee payer.
//!
//! zolana's transfer assembly (`zolana_client::assemble`) has no address
//! slots, so [`add_order_address`] assembles the transfer with zolana as usual
//! and then puts the address slot into the first padding slot, recomputing
//! exactly the values the slot changes: the slot's witness, the published
//! nullifiers, the input flags, the tree slots, the private transaction hash
//! and the public input hash. The construction follows zolana's compression
//! example (`sdk-tests/compression/sdk/src/instructions/create/proof.rs`).

mod assembly;

pub use assembly::{add_order_address, AddressPatch, WitnessRequest};

use solana_address::Address;
use zolana_client::ProofInputUtxo;
use zolana_hasher::primitives::hash_bytes;
use zolana_interface::{tree_slot::tree_id_field, ADDRESS_DOMAIN};
use zolana_keypair::{
    constants::BLINDING_LEN,
    hash::{owner_hash, right_align},
    NullifierKey, PublicKey,
};

use crate::swap::{OrderId, SwapError};

/// The tree whose nullifier tree holds every order address. Tree 0 exists on
/// every zolana deployment (the localnet fixture creates exactly tree 0), so
/// the user can recompute the address without knowing the market maker's tree.
pub const ORDER_ADDRESS_TREE: u16 = 0;

/// Input slots the order address adds to the market maker's fill transfer.
pub const ORDER_ADDRESS_SLOTS: usize = 1;

/// Domain tag hashed in front of the order id, so the seed of an order never
/// equals a seed another protocol derives from the same 16 bytes.
const ORDER_SEED_DOMAIN: [u8; 16] = *b"k-lend-rfq/order";

/// The address slot's seed (its UTXO blinding) for order `id`:
/// `hash_bytes(ORDER_SEED_DOMAIN || id)`, zolana's field hash of the 32 bytes
/// (two 31-byte big-endian chunks folded with Poseidon). The result is a
/// Poseidon output and so a canonical BN254 scalar, which the circuit requires
/// of a blinding; raw id bytes are never padded into a seed.
///
/// Errors with `SwapError::OrderAddressDerivation` if the hash fails.
fn order_address_seed(id: OrderId) -> Result<[u8; 32], SwapError> {
    [ORDER_SEED_DOMAIN.as_slice(), id.0.as_slice()]
        .concat()
        .try_into()
        .ok()
        .and_then(|preimage: [u8; 32]| hash_bytes(&preimage).ok())
        .ok_or(SwapError::OrderAddressDerivation { order: id })
}

/// The order address of order `id` filled by `market_maker_signer`, the
/// market maker's fee payer and signing key. See [`OrderAddress::new`] for
/// the derivation.
pub fn order_address(market_maker_signer: &Address, id: OrderId) -> Result<[u8; 32], SwapError> {
    Ok(OrderAddress::new(market_maker_signer, id)?.address)
}

/// The address slot a fill of an order carries, owned by the market maker's
/// signing key, the fill's fee payer. The circuit requires the owner to sign
/// the transaction.
#[derive(Clone, Debug)]
pub struct OrderAddress {
    id: OrderId,
    owner: PublicKey,
    /// The slot's UTXO.
    slot: ProofInputUtxo,
    /// The slot's nullifier, the order address.
    address: [u8; 32],
}

impl OrderAddress {
    /// The address slot of order `id` owned by `signer`. With the nullifier
    /// key of secret 0 (`nk0 = Poseidon(0)`) and
    /// `seed = order_address_seed(id)`:
    ///
    /// - `owner = owner_hash(signer, nk0)
    ///   = Poseidon(solana_owner_identity(signer), nk0)`;
    /// - `utxo_hash = Poseidon(ADDRESS_DOMAIN = 2, tree_id_field(0), 0, 0, 0,
    ///   Poseidon(0, 0), Poseidon(owner, seed))`: domain, tree, asset, amount,
    ///   data hash, ring hash and owner commitment of `ProofInputUtxo::hash`;
    /// - `address = Poseidon(utxo_hash, seed, 0)`, `NullifierKey::nullifier`
    ///   under the zero secret.
    ///
    /// The circuit checks an address slot exactly this way: zero asset,
    /// amount, data and nullifier secret, the owner a signer of the
    /// transaction, the blinding the seed.
    ///
    /// Errors with `SwapError::OrderAddressDerivation` if a hash fails.
    pub fn new(signer: &Address, id: OrderId) -> Result<Self, SwapError> {
        let derivation = || SwapError::OrderAddressDerivation { order: id };
        let seed = order_address_seed(id)?;
        // The zero secret, the nullifier key of every address slot.
        let key = NullifierKey::from_secret([0; BLINDING_LEN]);
        let nullifier_pubkey = key.pubkey().map_err(|_| derivation())?;
        let owner = PublicKey::from_ed25519(signer.as_array());
        let slot = ProofInputUtxo {
            domain: right_align(&ADDRESS_DOMAIN.to_be_bytes()),
            tree_id: tree_id_field(ORDER_ADDRESS_TREE),
            owner_hash: owner_hash(&owner, &nullifier_pubkey).map_err(|_| derivation())?,
            blinding: seed,
            ..ProofInputUtxo::default()
        };
        let utxo_hash = slot.hash().map_err(|_| derivation())?;
        let address = key.nullifier(&utxo_hash, &seed).map_err(|_| derivation())?;
        Ok(Self {
            id,
            owner,
            slot,
            address,
        })
    }

    /// The order address, the slot's nullifier.
    pub fn address(&self) -> [u8; 32] {
        self.address
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{anyhow, Result};
    use zolana_hasher::primitives::{is_canonical_bn254_scalar_be, solana_owner_identity};
    use zolana_keypair::hash::poseidon;

    use super::*;

    /// The order address written out as the circuit's Poseidon formula, with
    /// no zolana helper but the owner identity: `owner =
    /// Poseidon(solana_owner_identity(signer), Poseidon(0))`, `utxo_hash =
    /// Poseidon(2, tree_id 0, 0, 0, 0, Poseidon(0, 0), Poseidon(owner,
    /// seed))`, `address = Poseidon(utxo_hash, seed, 0)`.
    fn reference_address(signer: &Address, seed: &[u8; 32]) -> [u8; 32] {
        let zero = [0u8; 32];
        let hash = |inputs: &[&[u8]]| poseidon(inputs).unwrap_or_default();
        let nullifier_pubkey = hash(&[&zero]);
        let identity = solana_owner_identity(signer.as_array()).unwrap_or_default();
        let owner = hash(&[&identity, &nullifier_pubkey]);
        let mut domain = [0u8; 32];
        if let Some(last) = domain.last_mut() {
            *last = 2;
        }
        let ring_hash = hash(&[&zero, &zero]);
        let owner_utxo_hash = hash(&[&owner, seed]);
        let utxo_hash = hash(&[
            &domain,
            &zero,
            &zero,
            &zero,
            &zero,
            &ring_hash,
            &owner_utxo_hash,
        ]);
        hash(&[&utxo_hash, seed, &zero])
    }

    /// `order_address` equals the Poseidon formula of the address slot, and
    /// two order ids under one signer give two addresses.
    #[test]
    fn order_address_matches_the_slot_formula() -> Result<()> {
        let signer = Address::new_from_array([3; 32]);
        let first = OrderId([1; 16]);
        let second = OrderId([2; 16]);
        let want = reference_address(&signer, &order_address_seed(first)?);
        let got = order_address(&signer, first)?;
        assert_eq!(got, want, "order address of {first}");
        let other = order_address(&signer, second)?;
        assert_ne!(got, other, "orders {first} and {second} share an address");
        Ok(())
    }

    /// The seed is a canonical scalar distinct per id, not the padded id.
    #[test]
    fn order_seed_is_a_field_hash_of_the_id() -> Result<()> {
        let id = OrderId([0xff; 16]);
        let seed = order_address_seed(id)?;
        assert!(is_canonical_bn254_scalar_be(&seed), "seed {seed:?}");
        let padded: [u8; 32] = [[0u8; 16].as_slice(), id.0.as_slice()]
            .concat()
            .try_into()
            .map_err(|_| anyhow!("padded id length"))?;
        assert_ne!(seed, padded, "the seed is the padded id");
        assert_ne!(
            seed,
            order_address_seed(OrderId([0xfe; 16]))?,
            "two ids share a seed"
        );
        Ok(())
    }
}

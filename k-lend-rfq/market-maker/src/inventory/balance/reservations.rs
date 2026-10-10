//! The market maker's tracked UTXOs and which in-flight step holds each one. A
//! UTXO is reserved by at most one step at a time, and a UTXO whose
//! nullifier was seen spent is never tracked again, so no two steps can
//! spend the same input and a late sync cannot resurrect a spent one.

use dashmap::{mapref::entry::Entry, DashMap, DashSet};
use solana_address::Address;
use zolana_transaction::WalletUtxo;

use crate::{error::MarketMakerError, transactions::steps::StepId};

/// Operator-facing view of one tracked UTXO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InventoryUtxo {
    pub asset: Address,
    pub utxo_hash: [u8; 32],
    pub nullifier: [u8; 32],
    pub amount: u64,
    /// Held by an in-flight step.
    pub reserved: bool,
}

/// A market maker UTXO and, once the indexer has placed it, its leaf index.
#[derive(Clone)]
pub struct TrackedUtxo {
    pub wallet: WalletUtxo,
    /// `None` for the market maker's own outputs between landing and indexing;
    /// such a UTXO counts in the balance but cannot be proven yet.
    pub leaf_index: Option<u64>,
}

impl TrackedUtxo {
    pub fn utxo_hash(&self) -> [u8; 32] {
        self.wallet.utxo_hash
    }

    pub fn asset(&self) -> Address {
        self.wallet.utxo.asset.asset
    }

    pub fn amount(&self) -> u64 {
        self.wallet.utxo.amount
    }
}

/// Tracked UTXOs keyed by commitment, their reservations, and the
/// nullifiers seen spent.
#[derive(Default)]
pub struct Reservations {
    utxos: DashMap<[u8; 32], TrackedUtxo>,
    /// Commitment to the step that holds it.
    reserved: DashMap<[u8; 32], StepId>,
    spent_nullifiers: DashSet<[u8; 32]>,
}

impl Reservations {
    /// Tracks `utxo`; returns whether it was new. An already tracked UTXO
    /// only gains a leaf index it lacked; a spent one is ignored.
    pub fn insert(&self, utxo: TrackedUtxo) -> bool {
        if self.spent_nullifiers.contains(&utxo.wallet.nullifier) {
            return false;
        }
        match self.utxos.entry(utxo.utxo_hash()) {
            Entry::Occupied(mut existing) => {
                let existing = existing.get_mut();
                existing.leaf_index = existing.leaf_index.or(utxo.leaf_index);
                false
            }
            Entry::Vacant(vacant) => {
                vacant.insert(utxo);
                true
            }
        }
    }

    /// Whether the UTXO with commitment `utxo_hash` is tracked.
    pub fn tracks(&self, utxo_hash: &[u8; 32]) -> bool {
        self.utxos.contains_key(utxo_hash)
    }

    /// The tracked UTXO with commitment `utxo_hash`.
    pub fn get(&self, utxo_hash: &[u8; 32]) -> Option<TrackedUtxo> {
        self.utxos.get(utxo_hash).map(|entry| entry.value().clone())
    }

    /// Reserves every UTXO of `utxo_hashes` for `step`, or none of them: errors
    /// with `MarketMakerError::UtxoNotTracked` for an unknown UTXO and
    /// `MarketMakerError::UtxoReserved` for one another step holds.
    /// Re-reserving for the same step is allowed.
    pub fn reserve(&self, step: StepId, utxo_hashes: &[[u8; 32]]) -> Result<(), MarketMakerError> {
        for hash in utxo_hashes {
            if !self.utxos.contains_key(hash) {
                return Err(MarketMakerError::UtxoNotTracked(*hash));
            }
            if self
                .reserved
                .get(hash)
                .is_some_and(|holder| *holder != step)
            {
                return Err(MarketMakerError::UtxoReserved(*hash));
            }
        }
        for hash in utxo_hashes {
            self.reserved.insert(*hash, step);
        }
        Ok(())
    }

    /// Releases every UTXO `step` holds.
    pub fn release(&self, step: StepId) {
        self.reserved.retain(|_, holder| *holder != step);
    }

    /// Stops tracking the spent `utxo_hashes` and remembers their
    /// nullifiers so no later sync re-adds them.
    pub fn remove_spent(&self, utxo_hashes: &[[u8; 32]]) {
        for hash in utxo_hashes {
            if let Some((_, utxo)) = self.utxos.remove(hash) {
                self.spent_nullifiers.insert(utxo.wallet.nullifier);
            }
            self.reserved.remove(hash);
        }
    }

    /// Removes every tracked UTXO whose nullifier `is_spent`; returns how
    /// many.
    pub fn remove_spent_nullifiers(&self, is_spent: impl Fn(&[u8; 32]) -> bool) -> usize {
        let spent: Vec<[u8; 32]> = self
            .utxos
            .iter()
            .filter(|entry| is_spent(&entry.wallet.nullifier))
            .map(|entry| *entry.key())
            .collect();
        self.remove_spent(&spent);
        spent.len()
    }

    /// The nullifiers of every tracked UTXO.
    pub fn nullifiers(&self) -> Vec<[u8; 32]> {
        self.utxos
            .iter()
            .map(|entry| entry.wallet.nullifier)
            .collect()
    }

    /// The indexed UTXOs of `asset` as wallet UTXOs, reserved ones
    /// included.
    pub fn spendable(&self, asset: &Address) -> Vec<WalletUtxo> {
        self.utxos
            .iter()
            .filter(|entry| entry.asset() == *asset)
            .filter_map(|entry| {
                let leaf_index = entry.leaf_index?;
                let mut wallet = entry.wallet.clone();
                wallet.leaf_index = leaf_index;
                Some(wallet)
            })
            .collect()
    }

    /// True if `utxo` (keyed by `utxo_hash`) holds `asset` and is not
    /// reserved by any step, indexed or not. This is what `unreserved_balance`
    /// sums.
    fn is_unreserved(&self, asset: &Address, utxo_hash: &[u8; 32], utxo: &TrackedUtxo) -> bool {
        utxo.asset() == *asset && !self.reserved.contains_key(utxo_hash)
    }

    /// UTXOs of `asset` that a new step can select: unreserved
    /// (`is_unreserved`) and indexed (`leaf_index` is known). Excludes
    /// reserved UTXOs and UTXOs not yet indexed.
    pub fn available(&self, asset: &Address) -> Vec<TrackedUtxo> {
        self.utxos
            .iter()
            .filter(|entry| {
                self.is_unreserved(asset, entry.key(), entry.value()) && entry.leaf_index.is_some()
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Whether a step holds any UTXO of `asset`.
    pub fn in_flight(&self, asset: &Address) -> bool {
        self.utxos
            .iter()
            .any(|entry| entry.asset() == *asset && self.reserved.contains_key(entry.key()))
    }

    /// Whether any UTXO of `asset` lacks a leaf index.
    pub fn unindexed(&self, asset: &Address) -> bool {
        self.utxos
            .iter()
            .any(|entry| entry.asset() == *asset && entry.leaf_index.is_none())
    }

    /// Sum of the amounts of the UTXOs of `asset` no step holds, indexed or
    /// not. A UTXO without a leaf index is the market maker's own output of a
    /// landed step: it cannot be selected until sync indexes it, but it will
    /// be, so admission (`PendingBalance::queue`) counts it and scheduling
    /// backlogs the operation until then (`Coordinator::waits_for_utxos`).
    /// Errors with `MarketMakerError::AmountOverflow` if the sum overflows
    /// `u64`.
    pub fn unreserved_balance(&self, asset: &Address) -> Result<u64, MarketMakerError> {
        self.utxos
            .iter()
            .filter(|entry| self.is_unreserved(asset, entry.key(), entry.value()))
            .try_fold(0u64, |total, entry| total.checked_add(entry.amount()))
            .ok_or(MarketMakerError::AmountOverflow {
                context: "unreserved balance",
            })
    }

    /// Sum of the amounts of every tracked UTXO of `asset`: includes reserved
    /// UTXOs and UTXOs not yet indexed. This is the market maker's total
    /// holding, not what it can spend now.
    ///
    /// The sum saturates at `u64::MAX` on purpose: it is an infallible view
    /// for range checks, rebalance sizing and error reports. The tracked
    /// UTXOs of one mint never exceed the mint's `u64` supply, so it does not
    /// saturate in practice.
    pub fn balance(&self, asset: &Address) -> u64 {
        self.utxos
            .iter()
            .filter(|entry| entry.asset() == *asset)
            .fold(0u64, |total, entry| total.saturating_add(entry.amount()))
    }

    /// Every tracked UTXO of `asset`, largest first (ties by commitment).
    pub fn utxos(&self, asset: &Address) -> Vec<InventoryUtxo> {
        let mut utxos: Vec<InventoryUtxo> = self
            .utxos
            .iter()
            .filter(|entry| entry.asset() == *asset)
            .map(|entry| InventoryUtxo {
                asset: entry.asset(),
                utxo_hash: entry.utxo_hash(),
                nullifier: entry.wallet.nullifier,
                amount: entry.amount(),
                reserved: self.reserved.contains_key(entry.key()),
            })
            .collect();
        utxos.sort_by_key(|utxo| (std::cmp::Reverse(utxo.amount), utxo.utxo_hash));
        utxos
    }
}

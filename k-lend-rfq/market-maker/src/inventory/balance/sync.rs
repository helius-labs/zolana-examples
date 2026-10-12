//! Indexer sync: decrypts the transactions and deposits addressed to the
//! market maker, verifies which outputs are spendable, and feeds them into
//! `Reservations`, removing the ones whose nullifiers are spent. Sync is the
//! only path by which a UTXO gains its leaf index.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

use solana_address::Address;
use tokio::{
    sync::{mpsc, Mutex},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use zolana_client::{AsyncRpc, AsyncZolanaIndexer};
use zolana_interface::{pda, state::SplAssetRegistry};
use zolana_transaction::{
    verify_spendable, AssetRegistry, DecryptionResult, ShieldedKeys, ShieldedTransaction,
};

use super::{
    pending::PendingBalance,
    reservations::{InsertOutcome, TrackedUtxo},
};

#[cfg(test)]
mod tests;
use crate::{api::Inner, error::MarketMakerError, transactions::coordinator::Event};

/// Page size asked of the indexer; a smaller page only costs round trips.
const PAGE_LIMIT: u32 = 1_000;
/// Nullifiers per spend query, to bound the request size.
const NULLIFIER_CHUNK: usize = 64;

/// Incremental scan state of the market maker's account: the view tags it
/// queries by, the indexer cursors, and everything decrypted so far.
pub struct AccountSync {
    keys: Arc<dyn ShieldedKeys + Send + Sync>,
    indexer: Arc<AsyncZolanaIndexer>,
    pending: Arc<PendingBalance>,
    registry: Arc<RwLock<AssetRegistry>>,
    tags: Vec<[u8; 32]>,
    transactions_cursor: Option<Vec<u8>>,
    deposits_cursor: Option<Vec<u8>>,
    spend_cursors: HashMap<[u8; 32], Option<Vec<u8>>>,
    decrypted: DecryptionResult,
}

/// The pages one scan fetched and the provisional cursor after them, held
/// apart from `AccountSync` until the whole pass succeeds.
struct Scanned {
    transactions: Vec<ShieldedTransaction>,
    cursor: Option<Vec<u8>>,
}

/// UTXOs newly tracked and removed as spent by one sync pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub inserted: usize,
    /// Existing receipts that gained a leaf index and became selectable.
    pub indexed: usize,
    pub removed: usize,
}

impl SyncOutcome {
    /// Whether the pass changed the inventory, so scheduling may progress.
    pub fn changed(&self) -> bool {
        self.inserted > 0 || self.indexed > 0 || self.removed > 0
    }
}

impl AccountSync {
    /// Scans by the market maker's two tags: its confidential view tag and the
    /// x coordinate of its viewing public key.
    pub fn new(
        keys: Arc<dyn ShieldedKeys + Send + Sync>,
        indexer: Arc<AsyncZolanaIndexer>,
        pending: Arc<PendingBalance>,
        registry: Arc<RwLock<AssetRegistry>>,
    ) -> Result<Self, MarketMakerError> {
        let address = keys.address()?;
        let tags = vec![address.confidential_view_tag()?, address.viewing_pubkey.x()];
        Ok(Self {
            keys,
            indexer,
            pending,
            registry,
            tags,
            transactions_cursor: None,
            deposits_cursor: None,
            spend_cursors: HashMap::new(),
            decrypted: DecryptionResult::default(),
        })
    }

    /// Restarts the scan from the beginning on the next pass.
    pub fn rescan(&mut self) {
        self.transactions_cursor = None;
        self.deposits_cursor = None;
        self.spend_cursors.clear();
        self.decrypted = DecryptionResult::default();
    }

    /// One pass: fetches new transactions and deposits past the cursors and the
    /// transactions spending tracked nullifiers, decrypts them, and updates
    /// `Reservations`. Zero-amount UTXOs are not tracked. Errors with
    /// `MarketMakerError::Sync` on an indexer failure, or with the decryption
    /// or spendability check's error.
    ///
    /// Invariant: a pass either advances every cursor and applies every page,
    /// or changes nothing. Only newly decoded or reconstructed notes are
    /// verified; live notes and inputs needed by pending merges are retained.
    /// Spent-note tombstones prevent late pages from restoring spent funds.
    ///
    /// Every page is fetched against provisional cursors
    /// and decrypted into a copy of the accumulated result; only once all
    /// fetches (spends included), decryption and the spendability check
    /// succeeded are the cursors and the result committed to `self` and the
    /// UTXOs applied to `Reservations`. A failed or cancelled pass therefore
    /// leaves the cursors where they were, and the next pass refetches the
    /// same pages instead of skipping them.
    pub async fn run_once(&mut self) -> Result<SyncOutcome, MarketMakerError> {
        let transactions = self.transactions().await?;
        let deposits = self.deposits().await?;
        let mut spend_cursors = self.spend_cursors.clone();
        let mut fetched = transactions.transactions;
        fetched.extend(deposits.transactions);
        fetched.extend(self.spends(&mut spend_cursors).await?);
        let registry = self
            .registry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut decrypted = self.decrypted.clone();
        let known: HashSet<_> = decrypted.utxos.iter().map(|utxo| utxo.utxo_hash).collect();
        decrypted.extend(self.keys.as_ref(), &fetched, &registry)?;
        let delta = DecryptionResult {
            utxos: decrypted
                .utxos
                .iter()
                .filter(|utxo| !known.contains(&utxo.utxo_hash))
                .cloned()
                .collect(),
            spent_nullifiers: decrypted.spent_nullifiers.clone(),
            ..DecryptionResult::default()
        };
        let spendable = verify_spendable(self.keys.as_ref(), &delta)?;
        let verified: HashSet<_> = spendable.utxos().map(|utxo| utxo.utxo_hash).collect();
        let removed = self
            .pending
            .reservations
            .remove_spent_nullifiers(decrypted.spent_nullifiers.iter().copied());
        let mut inserted = 0;
        let mut indexed = 0;
        for utxo in spendable
            .balances
            .assets
            .into_iter()
            .flat_map(|balance| balance.utxos)
        {
            if utxo.utxo.amount == 0 {
                continue;
            }
            let tracked = TrackedUtxo {
                leaf_index: Some(utxo.leaf_index),
                wallet: utxo,
            };
            self.pending.inflow_landed(&tracked.utxo_hash());
            match self.pending.reservations.insert(tracked) {
                InsertOutcome::Added => inserted += 1,
                InsertOutcome::Indexed => indexed += 1,
                InsertOutcome::Unchanged | InsertOutcome::AlreadySpent => {}
            }
        }
        let merge_inputs: HashSet<_> = decrypted
            .pending_merges
            .iter()
            .flat_map(|merge| merge.nullifiers.iter().copied())
            .collect();
        decrypted.utxos.retain(|utxo| {
            let needed = merge_inputs.contains(&utxo.nullifier);
            (known.contains(&utxo.utxo_hash) || verified.contains(&utxo.utxo_hash) || needed)
                && (!self.pending.reservations.is_spent(&utxo.nullifier) || needed)
        });
        // Reservation tombstones preserve replay protection without re-scanning spends.
        decrypted.spent_nullifiers.clear();
        self.decrypted = decrypted;
        self.transactions_cursor = transactions.cursor;
        self.deposits_cursor = deposits.cursor;
        let live: HashSet<_> = self.pending.reservations.nullifiers().into_iter().collect();
        spend_cursors.retain(|nullifier, _| live.contains(nullifier));
        self.spend_cursors = spend_cursors;
        Ok(SyncOutcome {
            inserted,
            indexed,
            removed,
        })
    }

    /// Every page of transactions past `transactions_cursor`, and the cursor
    /// after the last page. Leaves `self` unchanged; `run_once` commits the
    /// cursor.
    async fn transactions(&self) -> Result<Scanned, MarketMakerError> {
        let mut scanned = Scanned {
            transactions: Vec::new(),
            cursor: self.transactions_cursor.clone(),
        };
        loop {
            let response = self
                .indexer
                .get_shielded_transactions_by_tags(
                    self.tags.clone(),
                    scanned.cursor.clone(),
                    Some(PAGE_LIMIT),
                    None,
                )
                .await
                .map_err(MarketMakerError::Sync)?;
            scanned
                .transactions
                .extend(response.transactions.into_iter().filter(|transaction| {
                    !transaction.proofless
                        && transaction.tx_viewing_pk.is_some()
                        && transaction.salt.is_some()
                }));
            if !advance(
                &mut scanned.cursor,
                response.next_cursor,
                response.scanned_through,
            ) {
                return Ok(scanned);
            }
        }
    }

    /// Every page of deposits past `deposits_cursor`, and the cursor after the
    /// last page. Leaves `self` unchanged; `run_once` commits the cursor.
    async fn deposits(&self) -> Result<Scanned, MarketMakerError> {
        let mut scanned = Scanned {
            transactions: Vec::new(),
            cursor: self.deposits_cursor.clone(),
        };
        loop {
            let response = self
                .indexer
                .get_encrypted_utxos_by_tags(
                    self.tags.clone(),
                    scanned.cursor.clone(),
                    Some(PAGE_LIMIT),
                    None,
                )
                .await
                .map_err(MarketMakerError::Sync)?;
            scanned.transactions.extend(
                response
                    .matches
                    .into_iter()
                    .filter_map(|matched| matched.into_proofless_transaction()),
            );
            if !advance(
                &mut scanned.cursor,
                response.next_cursor,
                response.scanned_through,
            ) {
                return Ok(scanned);
            }
        }
    }

    async fn spends(
        &self,
        cursors: &mut HashMap<[u8; 32], Option<Vec<u8>>>,
    ) -> Result<Vec<ShieldedTransaction>, MarketMakerError> {
        let unspent: Vec<[u8; 32]> = self.pending.reservations.nullifiers();
        let mut groups: BTreeMap<Option<Vec<u8>>, Vec<[u8; 32]>> = BTreeMap::new();
        for nullifier in unspent {
            groups
                .entry(cursors.get(&nullifier).cloned().flatten())
                .or_default()
                .push(nullifier);
        }
        let mut fetched = Vec::new();
        for (position, nullifiers) in groups {
            for chunk in nullifiers.chunks(NULLIFIER_CHUNK) {
                let mut cursor = position.clone();
                loop {
                    let response = self
                        .indexer
                        .get_shielded_transactions_by_nullifiers(
                            chunk.to_vec(),
                            cursor.clone(),
                            Some(PAGE_LIMIT),
                            None,
                        )
                        .await
                        .map_err(MarketMakerError::Sync)?;
                    fetched.extend(response.transactions.into_iter().filter(|tx| !tx.proofless));
                    if !advance(&mut cursor, response.next_cursor, response.scanned_through) {
                        break;
                    }
                }
                for nullifier in chunk {
                    cursors.insert(*nullifier, cursor.clone());
                }
            }
        }
        Ok(fetched)
    }
}

/// Moves `position` to the indexer's `scanned_through` (or `next_cursor`)
/// and returns whether another page follows.
fn advance(
    position: &mut Option<Vec<u8>>,
    next_cursor: Option<Vec<u8>>,
    scanned_through: Option<Vec<u8>>,
) -> bool {
    let more = next_cursor.is_some();
    if let Some(next) = scanned_through.or(next_cursor) {
        *position = Some(next);
    }
    more
}

/// Runs `run_once` every `interval` until `cancel`, sending `Event::Synced`
/// when a pass changed the inventory. Failures are logged and retried on the
/// next tick.
///
/// The first tick is one `interval` from now. Errors with
/// `MarketMakerError::DeadlineOverflow` when that instant overflows the clock
/// (`Settings::new` already rejects a zero interval).
pub fn spawn_sync(
    account: Arc<Mutex<AccountSync>>,
    interval: Duration,
    events: mpsc::UnboundedSender<Event>,
    cancel: CancellationToken,
) -> Result<JoinHandle<()>, MarketMakerError> {
    let first_tick = tokio::time::Instant::now().checked_add(interval).ok_or(
        MarketMakerError::DeadlineOverflow {
            context: "first sync tick",
        },
    )?;
    Ok(tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(first_tick, interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tick.tick() => match account.lock().await.run_once().await {
                    Ok(outcome) if outcome.changed() => {
                        let _ = events.send(Event::Synced);
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "sync failed"),
                },
            }
        }
    }))
}

impl Inner {
    /// One sync pass now, then wakes the coordinator. Errors with the pass's
    /// error or `MarketMakerError::CoordinatorStopped`.
    pub async fn sync(&self) -> Result<(), MarketMakerError> {
        self.account.lock().await.run_once().await?;
        self.runtime
            .events
            .send(Event::Synced)
            .map_err(|_| MarketMakerError::CoordinatorStopped)
    }
}

/// The zolana asset id of `mint`, read from its `SplAssetRegistry` PDA. Errors
/// with `MarketMakerError::AssetNotRegistered` when the PDA does not exist.
pub async fn registered_asset(rpc: &dyn AsyncRpc, mint: Address) -> Result<u64, MarketMakerError> {
    let account = rpc
        .get_account(pda::spl_asset_registry(&mint))
        .await
        .map_err(MarketMakerError::Rpc)?
        .ok_or(MarketMakerError::AssetNotRegistered { mint })?;
    SplAssetRegistry::from_account_bytes(&account.data)
        .map(|registry| registry.asset_id)
        .map_err(|error| MarketMakerError::AssetRegistry {
            mint,
            reason: format!("{error:?}"),
        })
}

/// An `AssetRegistry` of `mints`, each resolved by `registered_asset`.
pub async fn asset_registry(
    rpc: &dyn AsyncRpc,
    mints: impl IntoIterator<Item = Address>,
) -> Result<AssetRegistry, MarketMakerError> {
    let mut registry = AssetRegistry::default();
    for mint in mints {
        registry.insert(registered_asset(rpc, mint).await?, mint)?;
    }
    Ok(registry)
}

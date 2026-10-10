//! The market maker's public handle. `MarketMaker` is a cheap clone around
//! `Inner`; every state change goes through the coordinator task, and
//! dropping the last handle cancels it and every task it spawned.

use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard};

use anyhow::Result;
use solana_address::Address;
use solana_signature::Signature;
use solana_signer::Signer;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use zolana_client::{AsyncProverClient, AsyncRpc, AsyncSolanaRpc, AsyncZolanaIndexer};
use zolana_keypair::ShieldedAddress;
use zolana_transaction::{AssetRegistry, WalletUtxo};

use k_lend_rfq_sdk::{
    pair::{Pair, VaultState},
    swap::{Direction, Fill, Offer, SwapRequest},
    transfer::USER_OUTPUTS,
};

use crate::{
    config::{ConfigUpdate, IdentityConfig, MarketMakerConfig, Settings},
    error::MakerError,
    inventory::balance::{
        pending::PendingBalance,
        reservations::{InventoryUtxo, Reservations},
        sync::asset_registry,
        sync::{spawn_sync, AccountSync},
    },
    inventory::{consolidate::ConsolidateReceipt, rebalance::RebalanceKind},
    swap::{fill::MakerFill, orders::OpenOrders},
    transactions::{
        budget::{BudgetError, SwapBudget},
        coordinator::{Coordinator, Runtime, Services},
        kvault::check_vault,
        prove::{ProofQueue, ProofQueueConfig},
        send::SendQueue,
        Identity,
    },
};

/// A balance of both sides of a pair: `MarketMaker::holdings` returns the
/// maker's tracked balance, reserved and unindexed UTXOs included; the range
/// checks use the net balances (`PendingBalance::net_balance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holdings {
    pub collateral: u64,
    pub shares: u64,
}

/// A landed public vault operation of the maker (seeding or rebalance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultOperation {
    /// The vault before, as previewed.
    pub before: VaultState,
    /// The vault read after the operation was indexed.
    pub after: VaultState,
    /// Change of the vault's `token_available`.
    pub tokens: u64,
    /// Change of the vault's `shares_issued`.
    pub shares: u64,
    /// Shielded UTXOs the operation spent (0 for seeding).
    pub inputs: usize,
    pub signature: Signature,
}

/// Handle to a running market maker.
#[derive(Clone)]
pub struct MarketMaker {
    inner: Arc<Inner>,
}

/// State shared by the api calls; the coordinator owns everything mutable
/// that is not behind a lock here.
pub(crate) struct Inner {
    pub(crate) identity: Identity,
    pub(crate) runtime: Runtime,
    pub(crate) services: Services,
    /// The settings quotes read; the coordinator writes it on update.
    pub(crate) config: Arc<RwLock<Settings>>,
    /// The widest maker transfer next to the narrowest user transfer,
    /// computed once at start.
    pub(crate) max_maker_inputs: usize,
    /// The rent minimum of an empty account, fetched once at start. Each fill
    /// funds its order marker account with it.
    pub(crate) marker_lamports: u64,
    pub(crate) registry: Arc<RwLock<AssetRegistry>>,
    pub(crate) account: Arc<tokio::sync::Mutex<AccountSync>>,
    /// Orders issued by `quote` and not yet consumed by `fill`.
    pub(crate) orders: OpenOrders,
    background: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.runtime.cancel.cancel();
        self.runtime.tasks.close();
        for task in self
            .background
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            task.abort();
        }
    }
}

impl Inner {
    /// A read guard on the current settings; a poisoned lock is recovered,
    /// since writers replace the settings whole.
    pub(crate) fn settings(&self) -> RwLockReadGuard<'_, Settings> {
        self.config.read().unwrap_or_else(PoisonError::into_inner)
    }
}

impl MarketMaker {
    /// Validates `config`, checks every pair against its vault, reads the
    /// rent minimum and the asset registry, runs a first sync, sizes the
    /// transaction budget and starts the sync and coordinator tasks.
    ///
    /// The vault check (`check_vault`) reads each pair's vault once over the
    /// same pricing path a quote uses (vault, `GlobalConfig`, allocated
    /// reserves) and verifies that the vault's mint is `pair.token_mint` and
    /// that `pair.shares_mint`, `pair.token_vault` and `pair.authority` are
    /// the vault's PDAs. A missing or misconfigured vault therefore fails
    /// here rather than at the first quote: with `MakerError::VaultMissing`
    /// when the vault account does not exist (or the rpc does not show it
    /// yet), `MakerError::VaultState` when a pricing account is missing or
    /// does not parse, and `MakerError::Config(ConfigError::VaultMismatch)`
    /// naming the first field that differs.
    ///
    /// Errors with `MakerError::Config` on an invalid configuration, with
    /// the vault check errors above, with the rpc, registry or sync error,
    /// and with `MakerError::Budget` when not even the narrowest swap fits
    /// one transaction.
    pub async fn start(config: MarketMakerConfig) -> Result<Self, MakerError> {
        let MarketMakerConfig {
            connection,
            identity:
                IdentityConfig {
                    keys,
                    authority,
                    signer,
                },
            pairs,
            tokens,
            concurrency,
            quotes,
        } = config;
        let config = Settings::new(&concurrency, quotes, pairs, tokens)?;
        let identity = Identity {
            own: keys.address()?,
            payer: signer.pubkey(),
            keys,
            authority,
            signer,
            tree: connection.tree,
            tree_id: connection.tree_id,
        };
        let solana_rpc = Arc::new(AsyncSolanaRpc::new(connection.rpc_url));
        let rpc: Arc<dyn AsyncRpc> = solana_rpc.clone();
        let indexer = Arc::new(AsyncZolanaIndexer::new(connection.photon_url));
        let prover = Arc::new(
            connection
                .prover_url
                .map_or_else(AsyncProverClient::default, AsyncProverClient::new),
        );
        for pair in &config.pairs {
            check_vault(rpc.as_ref(), pair).await?;
        }
        let marker_lamports = rpc.get_minimum_balance_for_rent_exemption(0).await?;
        let registry = Arc::new(RwLock::new(
            asset_registry(rpc.as_ref(), config.assets()).await?,
        ));
        let pending = Arc::new(PendingBalance::new(Arc::new(Reservations::default())));
        let mut account = AccountSync::new(
            identity.keys.clone(),
            indexer.clone(),
            pending.clone(),
            registry.clone(),
        )?;
        account.run_once().await?;

        // A consolidation pays one output back to the maker.
        let budget = Arc::new(SwapBudget::new(identity.payer, identity.tree, 1)?);
        // Fails start when not even the narrowest swap fits one transaction;
        // each quote sizes its own user width later.
        budget.max_user_inputs(budget.narrowest_maker()?)?.ok_or(
            BudgetError::NoSupportedShape {
                inputs: 1,
                outputs: USER_OUTPUTS,
            },
        )?;
        let max_maker_inputs = budget.max_maker_inputs(&budget.narrowest_user_transfer()?)?;

        let proofs = Arc::new(ProofQueue::new(ProofQueueConfig {
            authority: identity.authority.clone(),
            indexer: indexer.clone(),
            prover,
            workers: concurrency.provers,
            payer: identity.payer,
        }));
        let sender = Arc::new(SendQueue::new(solana_rpc, identity.signer.clone()));
        let services = Services {
            rpc,
            indexer,
            pending,
            budget,
            proofs,
            sender,
        };
        let account = Arc::new(tokio::sync::Mutex::new(account));
        let (events, receiver) = mpsc::unbounded_channel();
        let runtime = Runtime {
            events,
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
        };

        let sync = spawn_sync(
            account.clone(),
            concurrency.sync_interval,
            runtime.events.clone(),
            runtime.cancel.clone(),
        )?;
        let shared_config = Arc::new(RwLock::new(config.clone()));
        let coordinator = Coordinator::new(
            config,
            shared_config.clone(),
            identity.clone(),
            runtime.clone(),
            services.clone(),
        );
        let coordinator = tokio::spawn(coordinator.run(receiver));

        Ok(Self {
            inner: Arc::new(Inner {
                identity,
                runtime,
                services,
                config: shared_config,
                max_maker_inputs,
                marker_lamports,
                registry,
                account,
                orders: OpenOrders::default(),
                background: Mutex::new(vec![sync, coordinator]),
            }),
        })
    }

    /// Applies `update` through the coordinator; see `Inner::update_config`.
    pub async fn update_config(&self, update: ConfigUpdate) -> Result<()> {
        Ok(self.inner.update_config(update).await?)
    }

    /// Cancels the background tasks and waits for them and every spawned
    /// task to finish. Fills awaiting a signature are released.
    pub async fn shutdown(&self) {
        self.inner.runtime.cancel.cancel();
        let background = std::mem::take(
            &mut *self
                .inner
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for task in background {
            if let Err(error) = task.await {
                tracing::warn!(%error, "background task ended abnormally");
            }
        }
        self.inner.runtime.tasks.close();
        self.inner.runtime.tasks.wait().await;
    }

    /// Operator-facing: the maker's balance of `asset` as quoting sees it, in
    /// base units of `asset`. Reflects the `Reservations` balance plus user
    /// payments of admitted fills not yet indexed minus payouts of fills not
    /// yet landed; the number quotes are range-checked against.
    pub fn net_balance(&self, asset: &Address) -> u64 {
        self.inner.services.pending.net_balance(asset)
    }

    /// Operator-facing: signatures of the automatic rebalances that landed, in
    /// landing order; look each one up on chain to audit the public kVault
    /// operations. Only rebalances the coordinator triggered on its own are
    /// listed (manual `rebalance_shares` / `rebalance_collateral` calls return
    /// their own `VaultOperation`), and only once they landed:
    /// `triggered_rebalances` also counts the queued and failed ones.
    pub fn rebalances(&self) -> Vec<Signature> {
        self.inner.services.pending.rebalances()
    }

    /// Operator-facing: number of automatic rebalances triggered since start,
    /// that is how many public kVault operations the maker has initiated on
    /// its own. Counted when the rebalance is queued, so it includes
    /// rebalances still in flight and ones that later fail; `rebalances` lists
    /// only the landed ones, so the difference is what is pending or failed.
    pub fn triggered_rebalances(&self) -> usize {
        self.inner.services.pending.triggered_rebalances()
    }

    /// Operator-facing: number of range checks since start that found a pair
    /// outside its target range and triggered no rebalance, because no
    /// rebalance fits both ranges or the vault preview refuses it. Counted on
    /// every such check, while the warning is logged only when the situation
    /// changes.
    pub fn range_refusals(&self) -> usize {
        self.inner.services.pending.range_refusals()
    }

    /// Operator-facing: the maker's shielded address, which users pay in
    /// their transfers and which receives every maker change output. Fixed at
    /// start from `IdentityConfig::keys`.
    pub fn identity(&self) -> ShieldedAddress {
        self.inner.identity.own
    }

    /// The maker's public key: fee payer of every swap and owner of its
    /// public token accounts.
    pub fn address(&self) -> Address {
        self.inner.identity.payer
    }

    /// Operator-facing: number of quoted orders a user can still fill, that
    /// is issued orders that are neither consumed nor expired at the time of
    /// the call. An order leaves this count on its first fill attempt
    /// (successful or not) or when its `order_ttl` elapses.
    pub fn open_orders(&self) -> usize {
        self.inner.orders.unexpired()
    }

    /// Operator-facing: number of fills that are proven and waiting for the
    /// user's signature. A fill leaves this count when it is settled, expires
    /// or is released at shutdown. Answered by the coordinator after every
    /// event queued before this call, so a fill whose `fill` call has
    /// returned is counted.
    pub async fn open_fills(&self) -> Result<usize> {
        let (reply, count) = tokio::sync::oneshot::channel();
        self.inner
            .runtime
            .events
            .send(crate::transactions::coordinator::Event::OpenFills { reply })
            .map_err(|_| MakerError::ShuttingDown)?;
        Ok(count.await.map_err(|_| MakerError::CoordinatorStopped)?)
    }

    /// Operator-facing: the maker's total holding of both sides of `pair`, in
    /// base units of each mint. Reflects every tracked UTXO in `Reservations`,
    /// reserved and not yet indexed ones included, so it is what the maker
    /// owns, not what it can spend now or what quotes are checked against
    /// (see `net_balance`).
    pub fn holdings(&self, pair: &Pair) -> Holdings {
        let reservations = &self.inner.services.pending.reservations;
        Holdings {
            collateral: reservations.balance(&pair.token_mint),
            shares: reservations.balance(&pair.shares_mint),
        }
    }

    /// Operator-facing: every tracked UTXO of `asset`, largest first, with its
    /// amount in base units and whether an in-flight step reserves it.
    /// Reflects `Reservations`, not-yet-indexed maker outputs included; use it
    /// to see how fragmented the inventory is before a `consolidate`.
    pub fn utxos(&self, asset: &Address) -> Vec<InventoryUtxo> {
        self.inner.services.pending.reservations.utxos(asset)
    }

    /// Operator-facing: the indexed UTXOs of `asset` as wallet UTXOs (leaf
    /// index and nullifier known), reserved ones included; amounts in base
    /// units. Unlike `utxos` it leaves out maker outputs sync has not indexed
    /// yet.
    pub fn spendable(&self, asset: &Address) -> Vec<WalletUtxo> {
        self.inner.services.pending.reservations.spendable(asset)
    }

    /// Runs one indexer sync now; see `Inner::sync`.
    pub async fn sync(&self) -> Result<()> {
        Ok(self.inner.sync().await?)
    }

    /// Prices a swap and opens an order; see `Inner::quote` for the checks.
    pub async fn quote(&self, pair: &Pair, direction: Direction, amount_in: u64) -> Result<Offer> {
        self.inner.quote(pair, direction, amount_in).await
    }

    /// Fills an open order; see `Inner::fill` for the checks.
    pub async fn fill(&self, pair: &Pair, request: &SwapRequest) -> Result<MakerFill> {
        self.inner.fill(pair, request).await
    }

    /// Co-signs and sends a fill with the user's signature and returns the
    /// landed signature; see `Coordinator::on_settle` and `co_sign`.
    pub async fn settle(&self, fill: &Fill, user_signature: Signature) -> Result<Signature> {
        Ok(self.inner.settle(fill, user_signature).await?)
    }

    /// Operator-facing: merges the available UTXOs of `asset` into the
    /// profile's parts in one transfer and returns once it landed, with the
    /// signature and the number of UTXOs spent and created. Errors with
    /// `MakerError::NothingToConsolidate` when fewer than two UTXOs are
    /// available; see `Inner::consolidate`.
    pub async fn consolidate(&self, asset: Address) -> Result<ConsolidateReceipt> {
        Ok(self.inner.consolidate(asset).await?)
    }

    /// Funds the shielded inventory from the maker's public accounts; see
    /// `Inner::seed_inventory`.
    pub async fn seed_inventory(
        &self,
        pair: &Pair,
        deposit: u64,
        collateral: u64,
    ) -> Result<VaultOperation> {
        Ok(self.inner.seed_inventory(pair, deposit, collateral).await?)
    }

    /// Operator-facing: manual deposit-side rebalance. Spends `collateral`
    /// (base units of `pair.token_mint`) from the shielded inventory, deposits
    /// it into the kVault and shields the minted shares; returns once indexed,
    /// with the vault before and after. Not listed in `rebalances`. Shares
    /// `Inner::rebalance` with `rebalance_collateral`.
    pub async fn rebalance_shares(&self, pair: &Pair, collateral: u64) -> Result<VaultOperation> {
        Ok(self
            .inner
            .rebalance(pair, RebalanceKind::Shares, collateral)
            .await?)
    }

    /// Operator-facing: manual withdraw-side rebalance. Spends `shares` (base
    /// units of `pair.shares_mint`) from the shielded inventory, withdraws
    /// them from the kVault and shields the collateral paid out; returns once
    /// indexed, with the vault before and after. Not listed in `rebalances`.
    /// Shares `Inner::rebalance` with `rebalance_shares`.
    pub async fn rebalance_collateral(&self, pair: &Pair, shares: u64) -> Result<VaultOperation> {
        Ok(self
            .inner
            .rebalance(pair, RebalanceKind::Collateral, shares)
            .await?)
    }
}

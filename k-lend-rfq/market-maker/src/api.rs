//! The market maker's public handle. `MarketMaker` is a cheap clone around
//! `Inner`; transaction steps run through the coordinator task, and
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
    error::MarketMakerError,
    inventory::balance::{
        pending::PendingBalance,
        reservations::{InventoryUtxo, Reservations},
        sync::asset_registry,
        sync::{spawn_sync, AccountSync},
    },
    inventory::{consolidate::ConsolidateReceipt, rebalance::RebalanceKind},
    swap::{fill::MarketMakerFill, orders::OpenOrders},
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
/// market maker's tracked balance, reserved and unindexed UTXOs included; the
/// range checks use the net balances (`PendingBalance::net_balance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holdings {
    pub collateral: u64,
    pub shares: u64,
}

/// A landed public vault operation of the market maker (seeding or rebalance).
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
    /// The widest market maker transfer next to the narrowest user transfer,
    /// computed once at start.
    pub(crate) max_market_maker_inputs: usize,
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
    /// Initializes the market maker and starts its background tasks.
    ///
    /// 1. Validate configuration, derive identities and create RPC, indexer and prover clients.
    /// 2. Check each pair against its vault and pricing accounts.
    /// 3. Load asset registrations and sync the initial inventory.
    /// 4. Compute transaction budgets and the market maker input limit.
    /// 5. Initialize proof and send queues and shared settings.
    /// 6. Start periodic inventory sync and the transaction coordinator.
    ///
    /// Missing vaults fail with `MarketMakerError::VaultMissing`; invalid pricing accounts
    /// with `MarketMakerError::VaultState`; mismatched pair addresses with
    /// `MarketMakerError::Config`. Configuration, registry, sync and transaction-budget
    /// errors are returned before a handle is created.
    pub async fn start(config: MarketMakerConfig) -> Result<Self, MarketMakerError> {
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

        // A consolidation pays one output back to the market maker.
        let budget = Arc::new(SwapBudget::new(identity.payer, identity.tree, 1)?);
        // Fails start when not even the narrowest swap fits one transaction;
        // each quote sizes its own user width later.
        budget
            .max_user_inputs(budget.narrowest_market_maker()?)?
            .ok_or(BudgetError::NoSupportedShape {
                inputs: 1,
                outputs: USER_OUTPUTS,
            })?;
        let max_market_maker_inputs =
            budget.max_market_maker_inputs(&budget.narrowest_user_transfer()?)?;

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
                max_market_maker_inputs,
                registry,
                account,
                orders: OpenOrders::default(),
                background: Mutex::new(vec![sync, coordinator]),
            }),
        })
    }

    /// Shuts down background services and waits for tracked tasks to finish.
    ///
    /// 1. Signal cancellation to the background tasks.
    /// 2. Await inventory sync and the coordinator, logging abnormal exits.
    /// 3. Close the task tracker and await all remaining tracked tasks.
    ///
    /// Fills awaiting a signature release their inventory and user-input reservations.
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

    /// Applies configuration changes to the running market maker.
    ///
    /// 1. Validate vaults and resolve asset registrations for any pairs being added.
    /// 2. For added pairs, extend the asset registry and rescan inventory before publication.
    /// 3. Ask the coordinator to validate and publish the update, and await its result.
    pub async fn update_config(&self, update: ConfigUpdate) -> Result<()> {
        Ok(self.inner.update_config(update).await?)
    }

    /// Refreshes the market maker's inventory from the indexer.
    ///
    /// 1. Fetch new transactions, deposits, and spends.
    /// 2. Decrypt fetched outputs and verify new notes, retaining live merge inputs.
    /// 3. Add newly discovered UTXOs and remove spent ones.
    /// 4. Notify the coordinator to retry queued operations.
    pub async fn sync(&self) -> Result<()> {
        Ok(self.inner.sync().await?)
    }

    /// Prices a swap and opens a recorded, expiring order.
    ///
    /// 1. Sweep expired orders and verify that the pair is served.
    /// 2. Read the vault state and price the amount with the configured fee.
    /// 3. Check projected balances against the inventory target ranges.
    /// 4. Determine the user input limit from available inventory and the transaction budget.
    /// 5. Record the pair, amounts, width and expiry, then return the offer and its fee.
    ///
    /// Recorded terms remain fixed until expiry, including after a configuration update.
    pub async fn quote(&self, pair: &Pair, direction: Direction, amount_in: u64) -> Result<Offer> {
        self.inner.quote(pair, direction, amount_in).await
    }

    /// Prepares a recorded order's swap transaction for the user's signature.
    ///
    /// 1. Verify the pair and consume its open, unexpired order.
    /// 2. Validate the user's input-owner signer, spent inputs, transfer shape and payment.
    /// 3. Reserve user inputs against overlapping fills and admit the inventory flows.
    /// 4. Select market maker inputs and prove its transfer with the SPP order address.
    /// 5. Combine the transfers and return the unsigned message, input nullifiers, change and deadline.
    ///
    /// Amounts and the deadline come from the recorded order. A failed fill attempt
    /// consumes the order, so a retry requires a fresh quote.
    pub async fn fill(&self, pair: &Pair, request: &SwapRequest) -> Result<MarketMakerFill> {
        self.inner.fill(pair, request).await
    }

    /// Settles a prepared fill using the user's signature.
    ///
    /// 1. Send the fill message and user signature to the coordinator.
    /// 2. Check that the fill is known, awaiting a signature, and unexpired.
    /// 3. Verify the expected user's signature, add the market maker's signature, and submit.
    /// 4. Await confirmation, update tracked inventory, and return the transaction signature.
    ///
    /// An invalid signature leaves the fill open for a valid signature before expiry.
    pub async fn settle(&self, fill: &Fill, user_signature: Signature) -> Result<Signature> {
        Ok(self.inner.settle(fill, user_signature).await?)
    }

    /// Reshapes available UTXOs for an asset using its configured inventory profile.
    ///
    /// 1. Verify the asset is configured and queue the consolidation.
    /// 2. Require at least two available UTXOs and select inputs within the transaction budget.
    /// 3. Plan replacement outputs from the inventory profile and reserve the selected inputs.
    /// 4. Prove, sign, and submit the self-transfer.
    /// 5. Await confirmation, update tracked inventory, and return the signature and UTXO counts.
    pub async fn consolidate(&self, asset: Address) -> Result<ConsolidateReceipt> {
        Ok(self.inner.consolidate(asset).await?)
    }

    /// Seeds private inventory from the market maker's public collateral balance.
    ///
    /// 1. Verify the pair is served, require a nonzero funding amount, and read the vault state.
    /// 2. Plan shielding of `collateral` using its configured inventory profile.
    /// 3. For a nonzero `deposit`, build a kVault deposit and shield the simulated share proceeds.
    /// 4. Check transaction size, sign, and submit the combined deposit and shielding instructions.
    /// 5. Await confirmation and indexing, sync inventory, and return the vault operation receipt.
    pub async fn seed_inventory(
        &self,
        pair: &Pair,
        deposit: u64,
        collateral: u64,
    ) -> Result<VaultOperation> {
        Ok(self.inner.seed_inventory(pair, deposit, collateral).await?)
    }

    /// Converts the market maker's existing private collateral into private kVault shares.
    ///
    /// 1. Verify the pair is served and sync inventory.
    /// 2. Queue the rebalance and read the vault state to plan the deposit and share outputs.
    /// 3. Select and reserve available collateral UTXOs within the transaction budget.
    /// 4. Prove unshielding of the required collateral, preserving private change.
    /// 5. Submit the unshield, kVault deposit, and shielding of simulated share proceeds atomically.
    /// 6. Await confirmation and indexing, sync inventory, and return the vault operation receipt.
    ///
    /// This manual rebalance is not added to the automatic `rebalances` list.
    pub async fn rebalance_shares(&self, pair: &Pair, collateral: u64) -> Result<VaultOperation> {
        Ok(self
            .inner
            .rebalance(pair, RebalanceKind::Shares, collateral)
            .await?)
    }

    /// Converts the market maker's existing private kVault shares into private collateral.
    ///
    /// 1. Verify the pair is served and sync inventory.
    /// 2. Queue the rebalance and read the vault state to plan the withdrawal and collateral outputs.
    /// 3. Select and reserve available share UTXOs within the transaction budget.
    /// 4. Prove unshielding of the required shares, preserving private change.
    /// 5. Submit the unshield, kVault withdrawal, and shielding of simulated collateral proceeds atomically.
    /// 6. Await confirmation and indexing, sync inventory, and return the vault operation receipt.
    ///
    /// This manual rebalance is not added to the automatic `rebalances` list.
    pub async fn rebalance_collateral(&self, pair: &Pair, shares: u64) -> Result<VaultOperation> {
        Ok(self
            .inner
            .rebalance(pair, RebalanceKind::Collateral, shares)
            .await?)
    }
}

impl MarketMaker {
    /// Operator-facing: the market maker's balance of `asset` as quoting sees
    /// it, in base units of `asset`. Reflects the `Reservations` balance plus
    /// user payments of admitted fills not yet indexed minus payouts of fills
    /// not yet landed; the number quotes are range-checked against.
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
    /// that is how many public kVault operations the market maker has initiated
    /// on its own. Counted when the rebalance is queued, so it includes
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

    /// Operator-facing: the market maker's shielded address, which users pay in
    /// their transfers and which receives every market maker change output.
    /// Fixed at start from `IdentityConfig::keys`.
    pub fn identity(&self) -> ShieldedAddress {
        self.inner.identity.own
    }

    /// The market maker's public key: fee payer of every swap and owner of its
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
            .map_err(|_| MarketMakerError::ShuttingDown)?;
        Ok(count
            .await
            .map_err(|_| MarketMakerError::CoordinatorStopped)?)
    }

    /// Operator-facing: the market maker's total holding of both sides of
    /// `pair`, in base units of each mint. Reflects every tracked UTXO in
    /// `Reservations`, reserved and not yet indexed ones included, so it is
    /// what the market maker owns, not what it can spend now or what quotes are
    /// checked against (see `net_balance`).
    pub fn holdings(&self, pair: &Pair) -> Holdings {
        let reservations = &self.inner.services.pending.reservations;
        Holdings {
            collateral: reservations.balance(&pair.token_mint),
            shares: reservations.balance(&pair.shares_mint),
        }
    }

    /// Operator-facing: every tracked UTXO of `asset`, largest first, with its
    /// amount in base units and whether an in-flight step reserves it. Reflects
    /// `Reservations`, not-yet-indexed market maker outputs included; use it to
    /// see how fragmented the inventory is before a `consolidate`.
    pub fn utxos(&self, asset: &Address) -> Vec<InventoryUtxo> {
        self.inner.services.pending.reservations.utxos(asset)
    }

    /// Operator-facing: the indexed UTXOs of `asset` as wallet UTXOs (leaf
    /// index and nullifier known), reserved ones included; amounts in base
    /// units. Unlike `utxos` it leaves out market maker outputs sync has not
    /// indexed yet.
    pub fn spendable(&self, asset: &Address) -> Vec<WalletUtxo> {
        self.inner.services.pending.reservations.spendable(asset)
    }
}

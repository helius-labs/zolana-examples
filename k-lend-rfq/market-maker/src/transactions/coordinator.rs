//! The coordinator: one task that owns the step table, the operation queue
//! and the running settings, and handles every event in turn. All state
//! changes of the market maker happen here, so no two steps can be scheduled
//! against the same inputs; slow rpc, indexer and prover work, including the
//! vault reads and previews of automatic rebalances, runs in spawned tasks
//! that report back through `Event`s.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, RwLock},
    time::Instant,
};

use solana_address::Address;
use solana_message::VersionedMessage;
use solana_signature::Signature;
use tokio::{
    sync::{mpsc, oneshot},
    time::MissedTickBehavior,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use zolana_client::{AsyncRpc, AsyncZolanaIndexer};

use k_lend_rfq_sdk::pair::VaultState;

use super::{
    budget::SwapBudget,
    confirm::{SpentCheck, StatusPoll},
    prove::{ProofQueue, ProvenStep},
    send::{SendOutcome, SendQueue},
    steps::{OperationId, StepId, Steps},
};
use crate::{
    api::Inner,
    config::{ConfigUpdate, Settings},
    error::MarketMakerError,
    inventory::balance::pending::{FillRanges, Outflow, PendingBalance},
    inventory::{
        consolidate::ConsolidateReceipt,
        rebalance::{CheckedPair, RangeWarning, RebalanceBackoff, RebalanceOrder},
    },
    swap::fill::{FillOrder, MarketMakerFill},
    transactions::Identity,
};

/// An api request that runs as one or more steps.
pub enum Operation {
    Fill(FillOrder),
    /// An explicit request to consolidate the asset.
    Consolidate(Address),
    Rebalance(RebalanceOrder),
}

impl Operation {
    /// The asset whose UTXOs the operation spends; operations on one asset
    /// are scheduled in queue order.
    pub fn asset(&self) -> Address {
        match self {
            Self::Fill(order) => order.asset,
            Self::Consolidate(asset) => *asset,
            Self::Rebalance(order) => order.spent_asset(),
        }
    }

    /// The amount of `asset` the operation commits while queued (0 for a
    /// consolidation, which keeps its value).
    pub fn amount(&self) -> u64 {
        match self {
            Self::Fill(order) => order.amount,
            Self::Consolidate(_) => 0,
            Self::Rebalance(order) => order.amount(),
        }
    }
}

/// What a successful operation answers with.
pub enum OperationOutcome {
    /// The swap message is ready for the user; answered before it lands.
    Filled(MarketMakerFill),
    /// The consolidation landed.
    Consolidated(ConsolidateReceipt),
    /// The rebalance landed; `vault_before` is the state it was previewed
    /// against.
    Rebalanced {
        receipt: ConsolidateReceipt,
        vault_before: Box<VaultState>,
    },
}

/// Where an operation's outcome is sent.
pub type OperationReply = oneshot::Sender<Result<OperationOutcome, MarketMakerError>>;

/// An operation with its reply channel and the steps it has used up.
pub struct QueuedOperation {
    pub id: OperationId,
    pub operation: Operation,
    pub reply: OperationReply,
    /// Aborted steps so far; capped by `OPERATION_ATTEMPTS`.
    pub attempts: u32,
}

/// Everything the coordinator loop reacts to.
pub enum Event {
    /// A new operation from the api.
    Operation(QueuedOperation),
    /// The user's signature for a parked fill.
    Settle {
        message: VersionedMessage,
        user_signature: Signature,
        reply: oneshot::Sender<Result<Signature, MarketMakerError>>,
    },
    /// A fill's co-signing deadline passed.
    Expire(StepId),
    /// A backoff retry of a step whose send did not reach the rpc.
    Send(StepId),
    /// A prove task finished.
    Proven {
        step: StepId,
        outcome: Result<ProvenStep, MarketMakerError>,
    },
    /// A status poll spawned by `spawn_poll_statuses` finished.
    Polled(StatusPoll),
    /// A send task finished.
    Sent { step: StepId, outcome: SendOutcome },
    /// A sync pass changed the inventory.
    Synced,
    /// A configuration update from the api.
    UpdateConfig {
        update: ConfigUpdate,
        reply: oneshot::Sender<Result<(), MarketMakerError>>,
    },
    /// Read of the number of fills waiting for the user's signature.
    OpenFills { reply: oneshot::Sender<usize> },
    /// The range preview spawned by `check_ranges` finished: the pairs it
    /// checked, ending at the first one with a rebalance order.
    RangesPreviewed(Vec<CheckedPair>),
}

/// What scheduling one queued operation did.
pub enum ScheduleOutcome {
    /// A step was admitted; the operation moves to `scheduled`.
    Scheduled,
    /// Not now: UTXOs of its asset are in flight or unindexed. It and every
    /// later operation on the asset stay queued, keeping per-asset order.
    Backlogged,
    /// Upkeep steps were preempted; schedule it again in the same pass.
    Retry,
}

/// The coordinator's channels and task bookkeeping, shared with the api.
#[derive(Clone)]
pub struct Runtime {
    pub events: mpsc::UnboundedSender<Event>,
    pub cancel: CancellationToken,
    pub tasks: TaskTracker,
}

/// The shared services every part of the market maker calls.
#[derive(Clone)]
pub struct Services {
    pub rpc: Arc<dyn AsyncRpc>,
    pub indexer: Arc<AsyncZolanaIndexer>,
    pub pending: Arc<PendingBalance>,
    pub budget: Arc<SwapBudget>,
    pub proofs: Arc<ProofQueue>,
    pub sender: Arc<SendQueue>,
}

/// The coordinator task's state.
pub struct Coordinator {
    /// The authoritative settings; copied to `shared_config` on change.
    pub config: Settings,
    /// The api's read copy of `config`.
    pub shared_config: Arc<RwLock<Settings>>,
    pub identity: Identity,
    pub runtime: Runtime,
    pub services: Services,
    pub steps: Steps,
    /// Operations not yet scheduled, in arrival order (requeued ones first).
    pub queue: VecDeque<QueuedOperation>,
    /// Operations with a step in flight, or awaiting their spent check.
    pub scheduled: HashMap<OperationId, QueuedOperation>,
    /// When the last api operation arrived; upkeep waits for idleness.
    pub last_operation: Instant,
    /// Set while a spawned status poll runs; at most one poll is in flight.
    pub poll_in_flight: bool,
    /// Inputs of discarded steps whose nullifier PDAs the next poll looks up.
    pub spent_checks: Vec<SpentCheck>,
    /// Aborted operations whose requeue waits for their spent check. They
    /// stay in `scheduled` until `finish_spent_check` requeues or fails them.
    pub awaiting_spent_check: HashSet<OperationId>,
    /// Per vault, what `check_ranges` last warned about: no rebalance fits
    /// both ranges, or the vault preview blocks the rebalance. The warning
    /// repeats only when it changes.
    pub range_warnings: HashMap<Address, RangeWarning>,
    /// Set while a range preview spawned by `check_ranges` runs; at most one
    /// is in flight.
    pub preview_in_flight: bool,
    /// Per vault, the last automatic rebalance's backoff (`check_ranges`).
    pub rebalance_backoff: HashMap<Address, RebalanceBackoff>,
}

impl Coordinator {
    /// A coordinator with an empty step table and queue.
    pub fn new(
        config: Settings,
        shared_config: Arc<RwLock<Settings>>,
        identity: Identity,
        runtime: Runtime,
        services: Services,
    ) -> Self {
        Self {
            config,
            shared_config,
            identity,
            runtime,
            services,
            steps: Steps::default(),
            queue: VecDeque::new(),
            scheduled: HashMap::new(),
            last_operation: Instant::now(),
            poll_in_flight: false,
            spent_checks: Vec::new(),
            awaiting_spent_check: HashSet::new(),
            range_warnings: HashMap::new(),
            preview_in_flight: false,
            rebalance_backoff: HashMap::new(),
        }
    }

    /// The coordinator loop: handles one event or status tick at a time.
    ///
    /// Co-signing a fill (`Event::Settle`) waits behind whatever the loop is
    /// doing, so the slow rpc work runs in spawned tasks that report back
    /// through events: proving and the fill's blockhash fetch
    /// (`Event::Proven`), sending (`Event::Sent`), status polling with the
    /// nullifier lookups of discarded steps (`Event::Polled`), and the vault
    /// reads and previews of automatic rebalances (`Event::RangesPreviewed`).
    /// The rebalance handlers await no rpc.
    pub async fn run(mut self, mut events: mpsc::UnboundedReceiver<Event>) {
        let mut status_tick = tokio::time::interval(self.config.status_interval);
        status_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.runtime.cancel.cancelled() => break,
                event = events.recv() => match event {
                    Some(event) => self.handle(event).await,
                    None => break,
                },
                _ = status_tick.tick() => self.on_status_tick().await,
            }
        }
        self.drain(events).await;
    }

    /// Spawns a status poll and runs the periodic work. Upkeep and rebalance
    /// triggers wait while an aborted operation awaits its spent check: the
    /// market maker is not idle until that operation is requeued or failed.
    async fn on_status_tick(&mut self) {
        self.spawn_poll_statuses();
        self.send_ready();
        if self.awaiting_spent_check.is_empty() {
            self.run_upkeep().await;
            self.check_ranges();
        }
        self.drop_retired();
    }

    /// Dispatches one event to its handler.
    async fn handle(&mut self, event: Event) {
        match event {
            Event::Operation(operation) => {
                self.last_operation = Instant::now();
                self.queue.push_back(operation);
                self.try_schedule().await;
            }
            Event::Settle {
                message,
                user_signature,
                reply,
            } => self.on_settle(&message, user_signature, reply).await,
            Event::Expire(step) => self.on_expire(step).await,
            Event::Send(step) => self.on_send_retry(step),
            Event::Proven { step, outcome } => self.on_proven(step, outcome).await,
            Event::Sent { step, outcome } => self.on_sent(step, outcome).await,
            Event::Polled(poll) => self.on_polled(poll).await,
            Event::Synced => self.try_schedule().await,
            Event::UpdateConfig { update, reply } => self.on_update_config(update, reply).await,
            Event::OpenFills { reply } => {
                let _ = reply.send(self.steps.awaiting_signature());
            }
            Event::RangesPreviewed(checked) => self.on_ranges_previewed(checked).await,
        }
    }

    /// Schedules queued operations in order until each is scheduled,
    /// rejected or backlogged.
    ///
    /// An operation behind a backlogged one on the same asset is not tried,
    /// so operations on one asset never overtake each other; operations on
    /// other assets proceed. A scheduled or rejected operation releases the
    /// amount it committed in the queue. Operations queued by handlers
    /// during the pass go before the ones kept back.
    pub async fn try_schedule(&mut self) {
        // After cancellation nothing new is built or proven; the queue stays
        // intact so `drain` can reject it with `ShuttingDown`.
        if self.runtime.cancel.is_cancelled() {
            return;
        }
        let mut pending: VecDeque<QueuedOperation> = std::mem::take(&mut self.queue);
        let mut blocked: HashSet<Address> = HashSet::new();
        let mut kept = VecDeque::new();
        // One preemption per operation per pass: a Retry that makes no
        // progress must not spin the coordinator, so a second Retry for the
        // same operation is handled like Backlogged.
        let mut retried: HashSet<OperationId> = HashSet::new();
        while let Some(queued) = pending.pop_front() {
            let asset = queued.operation.asset();
            if blocked.contains(&asset) {
                kept.push_back(queued);
                continue;
            }
            let outcome = match &queued.operation {
                Operation::Fill(order) => self.schedule_fill(queued.id, order).await,
                Operation::Consolidate(asset) => self.schedule_consolidate(queued.id, *asset).await,
                Operation::Rebalance(order) => self.schedule_rebalance(queued.id, order).await,
            };
            match outcome {
                Ok(ScheduleOutcome::Scheduled) => {
                    self.services
                        .pending
                        .unqueue(asset, queued.operation.amount());
                    self.scheduled.insert(queued.id, queued);
                }
                Ok(ScheduleOutcome::Retry) if retried.insert(queued.id) => {
                    pending.push_front(queued)
                }
                // Backlogged, or a second Retry for the same operation in
                // this pass.
                Ok(ScheduleOutcome::Backlogged | ScheduleOutcome::Retry) => {
                    blocked.insert(asset);
                    kept.push_back(queued);
                }
                Err(error) => self.reject(queued, error),
            }
        }
        // `self.queue` holds only what handlers queued during the pass.
        self.queue.append(&mut kept);
    }

    /// Whether nothing runs or waits: not shutting down, nothing queued and
    /// no step in flight.
    pub fn is_idle(&self) -> bool {
        !self.runtime.cancel.is_cancelled() && self.queue.is_empty() && self.steps.is_idle()
    }

    /// Shuts the coordinator down after cancellation.
    ///
    /// Contract: no new step is scheduled after cancellation (`try_schedule`
    /// returns early and `discard` fails instead of requeueing). Transactions
    /// already sent are polled until they confirm, fail or expire. Everything
    /// else is released with `MarketMakerError::ShuttingDown`: queued and newly
    /// arriving operations, settle and config requests, every step that
    /// never reached the chain (reservations released, pending balance
    /// settled, fill settle channel answered), every aborted operation
    /// still waiting for its spent check (it is held in `scheduled`), and
    /// every input reservation still held by a pending spent check
    /// (`release_spent_checks`). Status
    /// polls keep running as spawned tasks and report through
    /// `Event::Polled`.
    async fn drain(&mut self, mut events: mpsc::UnboundedReceiver<Event>) {
        self.services.proofs.close();
        self.reject_queued();
        let mut status_tick = tokio::time::interval(self.config.status_interval);
        while self.steps.has_pending_sends() {
            tokio::select! {
                Some(event) = events.recv() => self.drain_event(event).await,
                _ = status_tick.tick() => self.spawn_poll_statuses(),
            }
        }
        events.close();
        while let Ok(event) = events.try_recv() {
            self.drain_event(event).await;
        }
        self.reject_queued();
        self.release_unsent_steps();
        self.release_spent_checks();
        for operation in std::mem::take(&mut self.scheduled).into_values() {
            self.fail(operation, MarketMakerError::ShuttingDown);
        }
        self.awaiting_spent_check.clear();
    }

    /// Removes every step left after the sent ones reached a terminal state
    /// (typically `Proving` with a cancelled proof, or `AwaitingSignature`),
    /// releases what it holds and fails its operation with `ShuttingDown`.
    fn release_unsent_steps(&mut self) {
        let ids: Vec<StepId> = self.steps.in_flight().map(|step| step.id).collect();
        if ids.is_empty() {
            return;
        }
        tracing::info!(steps = ids.len(), "releasing unsent steps on shutdown");
        for id in ids {
            let Some(mut step) = self.steps.remove(id) else {
                continue;
            };
            self.release_step(&mut step, MarketMakerError::ShuttingDown);
            let Some(operation_id) = step.operation else {
                continue;
            };
            match self.scheduled.remove(&operation_id) {
                Some(operation) => self.fail(operation, MarketMakerError::ShuttingDown),
                None => self.services.pending.drop_fill(operation_id),
            }
        }
    }

    /// Fails every queued operation with `MarketMakerError::ShuttingDown`.
    fn reject_queued(&mut self) {
        for operation in std::mem::take(&mut self.queue) {
            self.reject(operation, MarketMakerError::ShuttingDown);
        }
    }

    /// Releases the amount a queued `operation` committed and fails it with
    /// `error`.
    fn reject(&self, operation: QueuedOperation, error: MarketMakerError) {
        self.services
            .pending
            .unqueue(operation.operation.asset(), operation.operation.amount());
        self.fail(operation, error);
    }

    /// Answers `operation` with `error` and drops its fill flows.
    pub fn fail(&self, operation: QueuedOperation, error: MarketMakerError) {
        self.services.pending.drop_fill(operation.id);
        let _ = operation.reply.send(Err(error));
    }

    /// Handles an event during shutdown: requests are refused with
    /// `MarketMakerError::ShuttingDown`, send and poll results are still
    /// applied.
    async fn drain_event(&mut self, event: Event) {
        match event {
            Event::Operation(operation) => self.reject(operation, MarketMakerError::ShuttingDown),
            Event::Settle { reply, .. } => {
                let _ = reply.send(Err(MarketMakerError::ShuttingDown));
            }
            Event::UpdateConfig { reply, .. } => {
                let _ = reply.send(Err(MarketMakerError::ShuttingDown));
            }
            Event::OpenFills { reply } => {
                let _ = reply.send(self.steps.awaiting_signature());
            }
            Event::Sent { step, outcome } => self.on_sent(step, outcome).await,
            Event::Polled(poll) => self.on_polled(poll).await,
            Event::Proven { .. }
            | Event::Expire(_)
            | Event::Send(_)
            | Event::Synced
            | Event::RangesPreviewed(_) => {}
        }
    }
}

impl Inner {
    /// Admits `operation` and waits for its outcome.
    ///
    /// Checks, in order: the market maker is not shutting down
    /// (`MarketMakerError::ShuttingDown`); a fill pays a non-zero amount
    /// (`MarketMakerError::AmountZero`); a fill's flows keep both assets in
    /// their ranges against the net balance (`PendingBalance::queue_fill`,
    /// `SwapError::OutsideTargetRange`); the unreserved balance covers the
    /// amount net of queued operations (`PendingBalance::queue`,
    /// `MarketMakerError::InsufficientBalance`). An admission that fails leaves
    /// no pending entry behind.
    pub async fn operation(
        &self,
        operation: Operation,
    ) -> Result<OperationOutcome, MarketMakerError> {
        if self.runtime.cancel.is_cancelled() {
            return Err(MarketMakerError::ShuttingDown);
        }
        if matches!(&operation, Operation::Fill(order) if order.amount == 0) {
            return Err(MarketMakerError::AmountZero);
        }
        let pending = &self.services.pending;
        let id = pending.next_operation();
        if let Operation::Fill(order) = &operation {
            pending.queue_fill(
                id,
                order.inflow.clone(),
                Outflow {
                    asset: order.asset,
                    amount: order.amount,
                },
                {
                    let settings = self.settings();
                    FillRanges {
                        inflow: settings.range(&order.inflow.asset),
                        outflow: settings.range(&order.asset),
                    }
                },
            )?;
        }
        let asset = operation.asset();
        let amount = operation.amount();
        if let Err(error) = pending.queue(asset, amount) {
            pending.drop_fill(id);
            return Err(error);
        }
        let (reply, outcome) = oneshot::channel();
        let queued = QueuedOperation {
            id,
            operation,
            reply,
            attempts: 0,
        };
        if self.runtime.events.send(Event::Operation(queued)).is_err() {
            pending.unqueue(asset, amount);
            pending.drop_fill(id);
            return Err(MarketMakerError::ShuttingDown);
        }
        outcome
            .await
            .map_err(|_| MarketMakerError::CoordinatorStopped)?
    }
}

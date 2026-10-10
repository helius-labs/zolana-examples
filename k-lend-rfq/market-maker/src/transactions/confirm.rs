//! Confirmation and failure handling of sent steps. A failed step's
//! operation is only rerun once its inputs are known to be unspent on chain,
//! so no operation, in particular no public vault rebalance, runs twice.

use std::{collections::HashSet, time::Duration};

use solana_address::Address;
use solana_signature::Signature;
use solana_transaction_status_client_types::TransactionStatus;
use zolana_client::{AsyncRpc, IndexerPollConfig};
use zolana_interface::{error::ShieldedPoolError, pda};

use k_lend_rfq_sdk::swap::SwapError;

use super::{
    coordinator::{Coordinator, Event, OperationOutcome, QueuedOperation, Services},
    prove::PROVE_ATTEMPTS,
    send::{classify, CustomError, SendQueue, Sent, StepStatus},
    steps::{OperationId, Step, StepId, StepKind, StepState},
};
use crate::{
    error::MarketMakerError, inventory::balance::reservations::TrackedUtxo,
    inventory::consolidate::ConsolidateReceipt,
};

/// Steps an operation may run (each with fresh inputs and proof) before it
/// fails with `MarketMakerError::OperationFailed`; bounds the retries of an
/// operation that fails for a persistent reason.
const OPERATION_ATTEMPTS: u32 = 3;
/// Sends of one step before it is aborted: expired sends (each resent on a
/// fresh blockhash) before `MarketMakerError::NotLanded`, and consecutive sends
/// that never reached the rpc (`SendOutcome::NotSent`) before their error.
pub const SEND_ATTEMPTS: usize = 5;
/// zolana `ShieldedPoolError` codes after which proving the same inputs
/// again can succeed: the proof referenced a root the tree has moved past.
/// Any other failure aborts the step.
const REPROVE_CODES: [u32; 2] = [
    ShieldedPoolError::TransactProofVerificationFailed as u32,
    ShieldedPoolError::StaleNullifierRoot as u32,
];
/// The most addresses one `getMultipleAccounts` request accepts
/// (`MAX_MULTIPLE_ACCOUNTS` in agave `rpc-client-types` `request.rs`).
const ACCOUNTS_BATCH: usize = 100;

/// What an aborted step does to its operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    /// Run the operation again, if its inputs are unspent and attempts
    /// remain.
    Requeue,
    /// Fail the operation now.
    Fail,
}

impl StepKind {
    /// What an abort of a step of this kind does to its operation: a fill's
    /// user signature binds its transaction, so it fails; other steps are
    /// requeued.
    pub fn retry(self) -> Retry {
        match self {
            Self::Fill => Retry::Fail,
            Self::Consolidate | Self::Rebalance => Retry::Requeue,
        }
    }
}

/// The inputs of a discarded step whose nullifier PDAs the next status poll
/// looks up. `operation` is set when the step's operation waits for the
/// result before it is requeued or failed.
pub struct SpentCheck {
    /// The discarded step. If its transaction may still land, the
    /// reservations hold its inputs under this id until the check resolves,
    /// so no new step can select an input that transaction may spend.
    pub step: StepId,
    pub operation: Option<OperationId>,
    /// Each tracked input as `(utxo hash, nullifier PDA)`.
    pub inputs: Vec<([u8; 32], Address)>,
}

/// A sent step as the poll saw it: the sends it had when the poll started and
/// their statuses, in the same order.
pub struct PolledStep {
    pub id: StepId,
    pub sends: Vec<Sent>,
    pub statuses: Vec<Option<TransactionStatus>>,
}

/// The statuses of every sent step and the block height they were read at,
/// which decides whether an unlanded send's blockhash has expired.
pub struct PolledStatuses {
    pub steps: Vec<PolledStep>,
    pub block_height: u64,
}

/// The result of one spawned status poll, carried back by `Event::Polled`.
pub struct StatusPoll {
    pub statuses: Result<PolledStatuses, MarketMakerError>,
    /// The spent checks the poll resolved, handed back so the loop can finish
    /// the decisions waiting on them.
    pub checks: Vec<SpentCheck>,
    /// The utxo hashes among `checks` whose nullifier PDA exists on-chain.
    pub spent: Result<Vec<[u8; 32]>, MarketMakerError>,
}

/// What `Coordinator::discard` does with the operation of an aborted step,
/// decided without rpc from the step's retry policy, the operation's attempt
/// count and the step's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposal {
    /// `Retry::Fail`, or `OPERATION_ATTEMPTS` reached: fail with
    /// `MarketMakerError::OperationFailed`.
    Exhausted,
    /// `count` of the step's inputs are no longer tracked (sync saw their
    /// nullifiers on chain): fail with `MarketMakerError::InputsSpent`.
    InputsSpent { count: usize },
    /// Requeue now: the step had no tracked inputs, or the market maker is
    /// shutting down (`requeue` then fails it with
    /// `MarketMakerError::ShuttingDown`).
    Requeue,
    /// Look up the nullifiers of the tracked inputs first
    /// (`finish_spent_check` decides).
    AwaitSpentCheck,
}

/// What `Disposal::of` decides from.
#[derive(Clone, Copy, Debug)]
struct DisposalInput {
    /// The aborted step's retry policy.
    retry: Retry,
    /// Steps the operation has used up, the aborted one included.
    attempts: u32,
    /// The step's inputs the reservations no longer track.
    untracked: usize,
    /// The step's inputs the reservations still track.
    tracked: usize,
    /// Whether the market maker is shutting down.
    cancelled: bool,
}

impl Disposal {
    /// The checks of `Coordinator::discard`, in its order: exhaustion, then
    /// untracked inputs, then cancellation or no tracked inputs (requeue),
    /// else the spent check.
    fn of(input: DisposalInput) -> Self {
        let DisposalInput {
            retry,
            attempts,
            untracked,
            tracked,
            cancelled,
        } = input;
        if retry == Retry::Fail || attempts >= OPERATION_ATTEMPTS {
            Self::Exhausted
        } else if untracked > 0 {
            Self::InputsSpent { count: untracked }
        } else if cancelled || tracked == 0 {
            Self::Requeue
        } else {
            Self::AwaitSpentCheck
        }
    }
}

impl Coordinator {
    /// Spawns one status poll unless one is in flight (`poll_in_flight`).
    ///
    /// The step list and the pending spent checks are collected on the loop;
    /// the rpc calls (`getSignatureStatuses`, `getBlockHeight` and
    /// `getMultipleAccounts` on nullifier PDAs) run in the spawned task so a
    /// slow rpc does not delay co-signing. The task always answers with
    /// `Event::Polled`, also when a call fails, which clears the flag. The poll
    /// is not cancelled on shutdown: `drain` keeps polling sent transactions.
    pub fn spawn_poll_statuses(&mut self) {
        if self.poll_in_flight {
            return;
        }
        let sent: Vec<(StepId, Vec<Sent>)> = self
            .steps
            .sent()
            .map(|step| (step.id, step.sends.clone()))
            .collect();
        if sent.is_empty() && self.spent_checks.is_empty() {
            return;
        }
        let checks = std::mem::take(&mut self.spent_checks);
        self.poll_in_flight = true;
        let sender = self.services.sender.clone();
        let rpc = self.services.rpc.clone();
        let events = self.runtime.events.clone();
        self.runtime.tasks.spawn(async move {
            let (statuses, spent) = futures::join!(
                poll_sent(&sender, sent),
                spent_inputs(rpc.as_ref(), &checks)
            );
            let _ = events.send(Event::Polled(StatusPoll {
                statuses,
                checks,
                spent,
            }));
        });
    }

    /// Applies a finished status poll: drops spent inputs and finishes the
    /// decisions waiting on them, then classifies each polled step. A failed
    /// nullifier lookup puts its checks back for the next poll; a failed
    /// status lookup is retried on the next tick. After cancellation, once
    /// no send is pending, the remaining checks release their inputs
    /// (`release_spent_checks`).
    pub async fn on_polled(&mut self, poll: StatusPoll) {
        self.poll_in_flight = false;
        let StatusPoll {
            statuses,
            checks,
            spent,
        } = poll;
        let requeued = match spent {
            Ok(spent) => self.resolve_spent_checks(checks, spent),
            Err(error) => {
                tracing::warn!(%error, "nullifier pda check failed, retrying on the next poll");
                self.spent_checks.extend(checks);
                false
            }
        };
        match statuses {
            Ok(statuses) => self.apply_statuses(statuses).await,
            Err(error) => tracing::warn!(%error, "status poll failed"),
        }
        // During shutdown `drain` stops polling once no send is pending, so
        // the checks of steps aborted by this poll would never resolve.
        if self.runtime.cancel.is_cancelled() && !self.steps.has_pending_sends() {
            self.release_spent_checks();
        }
        if requeued {
            self.try_schedule().await;
        }
    }

    /// Applies each step's classified status: confirmed, failed (see
    /// `handle_failure`), expired (see `on_expired`) or still pending.
    async fn apply_statuses(&mut self, polled: PolledStatuses) {
        for PolledStep {
            id,
            sends,
            statuses,
        } in polled.steps
        {
            // The step may have changed while the poll ran (resent,
            // re-proven, aborted); its next poll sees the current sends.
            let unchanged = self
                .steps
                .get(id)
                .is_some_and(|step| step.state == StepState::Sent && step.sends == sends);
            if !unchanged {
                continue;
            }
            match classify(&sends, &statuses, polled.block_height) {
                StepStatus::Confirmed { signature } => self.on_confirmed(id, signature).await,
                StepStatus::Failed { reason, custom } => {
                    self.handle_failure(id, custom, MarketMakerError::TransactionFailed(reason))
                        .await
                }
                StepStatus::Expired => self.on_expired(id).await,
                StepStatus::Pending => {}
            }
        }
    }

    /// Removes the spent inputs from the reservations, releases the unspent
    /// inputs each checked step still holds, then finishes each operation
    /// waiting on its check. Returns whether one was requeued.
    fn resolve_spent_checks(&mut self, checks: Vec<SpentCheck>, spent: Vec<[u8; 32]>) -> bool {
        if !spent.is_empty() {
            tracing::warn!(
                utxos = spent.len(),
                "dropping utxos whose nullifiers are on-chain"
            );
            self.services.pending.reservations.remove_spent(&spent);
        }
        for check in &checks {
            self.services.pending.reservations.release(check.step);
        }
        let spent: HashSet<[u8; 32]> = spent.into_iter().collect();
        let mut requeued = false;
        for check in checks {
            let Some(operation) = check.operation else {
                continue;
            };
            let count = check
                .inputs
                .iter()
                .filter(|(hash, _)| spent.contains(hash))
                .count();
            requeued |= self.finish_spent_check(operation, count);
        }
        requeued
    }

    /// Finishes the deferred requeue decision of an aborted operation.
    ///
    /// When `spent > 0` inputs of the operation are spent on-chain, its
    /// transaction (or another spender) consumed them. Rerunning it would act
    /// twice, for a rebalance a second deposit or withdrawal, and the outputs
    /// of a landed transaction reach the inventory through sync anyway, so the
    /// operation fails with `MarketMakerError::InputsSpent`. Otherwise it is
    /// requeued. Returns whether it was requeued.
    fn finish_spent_check(&mut self, id: OperationId, spent: usize) -> bool {
        if !self.awaiting_spent_check.remove(&id) {
            return false;
        }
        let Some(operation) = self.scheduled.remove(&id) else {
            return false;
        };
        if spent > 0 {
            self.fail(operation, MarketMakerError::InputsSpent { count: spent });
            return false;
        }
        self.requeue(operation)
    }

    /// Puts an aborted operation back at the front of the queue, or fails it
    /// with `ShuttingDown` after cancellation. Returns whether it was queued.
    fn requeue(&mut self, operation: QueuedOperation) -> bool {
        if self.runtime.cancel.is_cancelled() {
            self.fail(operation, MarketMakerError::ShuttingDown);
            return false;
        }
        match self
            .services
            .pending
            .queue(operation.operation.asset(), operation.operation.amount())
        {
            Ok(()) => {
                self.queue.push_front(operation);
                true
            }
            Err(error) => {
                self.fail(operation, error);
                false
            }
        }
    }

    /// Handles a transaction that landed and failed or was rejected at
    /// preflight; `custom` is the failing instruction and its custom program
    /// error, if any. In order:
    /// - a fill whose market maker transfer failed with
    ///   `NullifierAlreadyQueued` (`CustomError::is_order_already_filled`)
    ///   fails with `SwapError::OrderAlreadyFilled`: the order address's
    ///   nullifier PDA exists, so another transaction already filled it;
    /// - a non-fill step re-proves when the code is in `REPROVE_CODES` and
    ///   prove attempts remain (`PROVE_ATTEMPTS`). This includes a rebalance
    ///   with a kVault tail: `spawn_prove` resolves its `TailShield` again
    ///   from a simulation of the new proof and replaces the tail, so a
    ///   stale root costs a prove attempt, not an operation attempt;
    /// - otherwise the step aborts with `error`, requeueing the operation or
    ///   failing it for a fill.
    pub async fn handle_failure(
        &mut self,
        id: StepId,
        custom: Option<CustomError>,
        error: MarketMakerError,
    ) {
        let Some(step) = self.steps.get(id) else {
            return;
        };
        let already_filled = custom
            .filter(|custom| custom.is_order_already_filled(step.kind))
            .and(step.fill.as_ref())
            .map(|fill| fill.order);
        if let Some(order) = already_filled {
            let error = MarketMakerError::Swap(SwapError::OrderAlreadyFilled { order });
            self.abort(id, error, Retry::Fail).await;
            return;
        }
        let reprove = custom.is_some_and(|custom| REPROVE_CODES.contains(&custom.code))
            && step.kind != StepKind::Fill
            && step.prove_attempts < PROVE_ATTEMPTS;
        if !reprove {
            let retry = step.kind.retry();
            self.abort(id, error, retry).await;
            return;
        }
        tracing::warn!(step = id, %error, "re-proving against fresh roots");
        if let Some(step) = self.steps.get_mut(id) {
            step.sends.clear();
            step.resend_failed = false;
        }
        self.spawn_prove(id);
    }

    /// Every send of step `id` expired unlanded. A fill fails (its order
    /// cannot be re-signed); another step is resent, unless its sends are
    /// exhausted (`SEND_ATTEMPTS`, or a rejected resend) or the market maker is
    /// shutting down, in which case it is aborted and requeued.
    /// The caller (`apply_statuses`) has just checked that the step exists.
    async fn on_expired(&mut self, id: StepId) {
        let Some(step) = self.steps.get(id) else {
            return;
        };
        let kind = step.kind;
        if kind == StepKind::Fill
            || step.resend_failed
            || step.sends.len() >= SEND_ATTEMPTS
            || self.runtime.cancel.is_cancelled()
        {
            self.abort(id, MarketMakerError::NotLanded, kind.retry())
                .await;
        } else {
            self.spawn_send(id);
        }
    }

    /// Step `id` landed as `signature`: its inputs leave the inventory, its
    /// own outputs enter it unindexed, its fill outflow is finished, and the
    /// waiting `settle` and operation callers are answered.
    async fn on_confirmed(&mut self, id: StepId, signature: Signature) {
        let Some(step) = self.steps.get_mut(id) else {
            return;
        };
        step.state = StepState::Confirmed;
        let operation = step.operation;
        let vault_before = step.vault_before.take();
        let inputs = step.inputs.clone();
        let expected_outputs = step.expected_outputs.clone();
        let settle = step.fill.as_mut().and_then(|fill| fill.settle.take());
        self.services.pending.settle(id);
        self.services.pending.reservations.remove_spent(&inputs);
        let outputs = expected_outputs.len();
        for wallet in expected_outputs {
            self.services.pending.reservations.insert(TrackedUtxo {
                wallet,
                leaf_index: None,
            });
        }
        if let Some(operation) = operation {
            self.services.pending.finish_fill(operation);
        }
        if let Some(settle) = settle {
            let _ = settle.send(Ok(signature));
        }
        if let Some(operation) = operation.and_then(|operation| self.scheduled.remove(&operation)) {
            let receipt = ConsolidateReceipt {
                signature,
                inputs: inputs.len(),
                outputs,
            };
            let outcome = match vault_before {
                Some(vault_before) => OperationOutcome::Rebalanced {
                    receipt,
                    vault_before: Box::new(vault_before),
                },
                None => OperationOutcome::Consolidated(receipt),
            };
            let _ = operation.reply.send(Ok(outcome));
        }
        self.steps.prune_confirmed();
        self.send_ready();
        self.try_schedule().await;
    }

    /// `discard`s step `id`, then schedules what its release made possible.
    pub async fn abort(&mut self, id: StepId, error: MarketMakerError, retry: Retry) {
        self.discard(id, error, retry).await;
        self.try_schedule().await;
    }

    /// Removes step `id`, releases what it holds (no rpc call: this runs on
    /// the coordinator loop) and decides what happens to its operation, in
    /// order:
    /// - no operation waits (upkeep, or a fill whose caller is gone): nothing;
    /// - `Retry::Fail` or `OPERATION_ATTEMPTS` reached: fails it now with
    ///   `MarketMakerError::OperationFailed`;
    /// - some of the step's inputs are no longer tracked: sync removed them
    ///   because their nullifiers are on chain, so the step's transaction (or
    ///   another spender) consumed them; fails it now with
    ///   `MarketMakerError::InputsSpent` counting the untracked inputs, since
    ///   rerunning it could act twice (for a rebalance, a second deposit or
    ///   withdrawal);
    /// - after cancellation: fails it now with
    ///   `MarketMakerError::ShuttingDown`;
    /// - `Retry::Requeue`: defers the decision to `finish_spent_check`, which
    ///   runs once the next status poll has looked up the nullifier PDAs of
    ///   the step's tracked inputs, because an operation whose inputs were
    ///   spent must not run again. Meanwhile the operation stays in
    ///   `scheduled`, so `drain` fails it with `ShuttingDown`. A step without
    ///   inputs is requeued at once.
    ///
    /// The tracked inputs are checked in every case, so inputs spent outside
    /// this market maker leave the reservations. A step whose transaction may
    /// still land (it reached `Sending`, or has sends) keeps its input
    /// reservations until that check resolves (`resolve_spent_checks`), so
    /// the inputs cannot be selected by a new step meanwhile; `drain` releases
    /// what is still held on shutdown (`release_spent_checks`). A step that
    /// never reached the rpc releases them at once.
    pub async fn discard(&mut self, id: StepId, error: MarketMakerError, retry: Retry) {
        tracing::warn!(step = id, %error, "aborting step");
        let Some(mut step) = self.steps.remove(id) else {
            return;
        };
        let inputs = self.tracked_inputs(&step.inputs);
        let untracked = step.inputs.len().saturating_sub(inputs.len());
        let reason = error.to_string();
        let may_land = !step.state.is_unsent() || !step.sends.is_empty();
        if may_land {
            self.release_pending(&mut step, error);
        } else {
            self.release_step(&mut step, error);
        }
        let Some(operation_id) = step.operation else {
            self.check_spent(id, None, inputs);
            return;
        };
        let Some(operation) = self.scheduled.get_mut(&operation_id) else {
            self.services.pending.drop_fill(operation_id);
            self.check_spent(id, None, inputs);
            return;
        };
        operation.attempts = operation.attempts.saturating_add(1);
        let attempts = operation.attempts;
        let disposal = Disposal::of(DisposalInput {
            retry,
            attempts,
            untracked,
            tracked: inputs.len(),
            cancelled: self.runtime.cancel.is_cancelled(),
        });
        if disposal == Disposal::AwaitSpentCheck {
            self.awaiting_spent_check.insert(operation_id);
            self.check_spent(id, Some(operation_id), inputs);
            return;
        }
        self.check_spent(id, None, inputs);
        let Some(operation) = self.scheduled.remove(&operation_id) else {
            return;
        };
        match disposal {
            Disposal::Exhausted => self.fail(
                operation,
                MarketMakerError::OperationFailed { attempts, reason },
            ),
            Disposal::InputsSpent { count } => {
                self.fail(operation, MarketMakerError::InputsSpent { count })
            }
            Disposal::Requeue | Disposal::AwaitSpentCheck => {
                self.requeue(operation);
            }
        }
    }

    /// Releases what a removed step holds: its UTXO reservations and what
    /// `release_pending` releases.
    pub fn release_step(&self, step: &mut Step, error: MarketMakerError) {
        self.services.pending.reservations.release(step.id);
        self.release_pending(step, error);
    }

    /// Releases a removed step's pending balance entries and answers the
    /// settle channel of a fill with `Err(error)`; its UTXO reservations stay.
    fn release_pending(&self, step: &mut Step, error: MarketMakerError) {
        self.services.pending.settle(step.id);
        if let Some(settle) = step.fill.as_mut().and_then(|fill| fill.settle.take()) {
            let _ = settle.send(Err(error));
        }
    }

    /// Releases the input reservations every pending spent check holds and
    /// drops the checks. For shutdown, after the last status poll: no check
    /// resolves any more, so nothing would release them otherwise.
    pub fn release_spent_checks(&mut self) {
        for check in std::mem::take(&mut self.spent_checks) {
            self.services.pending.reservations.release(check.step);
        }
    }

    /// The inputs still tracked by the reservations, each with the PDA its
    /// nullifier creates when spent.
    fn tracked_inputs(&self, inputs: &[[u8; 32]]) -> Vec<([u8; 32], Address)> {
        inputs
            .iter()
            .filter_map(|hash| self.services.pending.reservations.get(hash))
            .map(|utxo| {
                let tree_pda = pda::tree(utxo.wallet.tree_id);
                (
                    utxo.utxo_hash(),
                    pda::nullifier_pda(&tree_pda, &utxo.wallet.nullifier).0,
                )
            })
            .collect()
    }

    /// Queues the tracked `inputs` of discarded step `step` for the
    /// nullifier lookup of the next status poll. With nothing to look up,
    /// releases whatever the step still holds at once.
    fn check_spent(
        &mut self,
        step: StepId,
        operation: Option<OperationId>,
        inputs: Vec<([u8; 32], Address)>,
    ) {
        if inputs.is_empty() {
            self.services.pending.reservations.release(step);
            return;
        }
        self.spent_checks.push(SpentCheck {
            step,
            operation,
            inputs,
        });
    }
}

/// Fetches the current block height, then the statuses of every send in
/// `sent`. The two reads are sequential and in this order, as `classify`
/// requires: a send whose status is missing at a block height past its last
/// valid block height can then never land. Runs in the poll task, off the
/// coordinator loop.
async fn poll_sent(
    sender: &SendQueue,
    sent: Vec<(StepId, Vec<Sent>)>,
) -> Result<PolledStatuses, MarketMakerError> {
    if sent.is_empty() {
        return Ok(PolledStatuses {
            steps: Vec::new(),
            block_height: 0,
        });
    }
    let signatures: Vec<Signature> = sent
        .iter()
        .flat_map(|(_, sends)| sends.iter().map(|sent| sent.signature))
        .collect();
    let block_height = sender.block_height().await?;
    let statuses = sender.statuses(&signatures).await?;
    let mut statuses = statuses.into_iter();
    let steps = sent
        .into_iter()
        .map(|(id, sends)| PolledStep {
            id,
            statuses: statuses.by_ref().take(sends.len()).collect(),
            sends,
        })
        .collect();
    Ok(PolledStatuses {
        steps,
        block_height,
    })
}

/// Returns the utxo hashes in `checks` whose nullifier PDA exists, that is
/// whose utxo is spent on-chain. Runs in the poll task, off the coordinator
/// loop.
async fn spent_inputs(
    rpc: &dyn AsyncRpc,
    checks: &[SpentCheck],
) -> Result<Vec<[u8; 32]>, MarketMakerError> {
    let inputs: Vec<&([u8; 32], Address)> = checks.iter().flat_map(|check| &check.inputs).collect();
    let mut spent = Vec::new();
    for batch in inputs.chunks(ACCOUNTS_BATCH) {
        let pdas = batch.iter().map(|(_, pda)| *pda).collect();
        let accounts = rpc
            .get_multiple_accounts(pdas)
            .await
            .map_err(MarketMakerError::Rpc)?;
        spent.extend(
            batch
                .iter()
                .zip(accounts)
                .filter(|(_, account)| account.is_some())
                .map(|((hash, _), _)| *hash),
        );
    }
    Ok(spent)
}

/// Waits until `signature` is confirmed and the indexer has it, polling with
/// the zolana client's default indexer backoff. Errors with
/// `MarketMakerError::NotConfirmed` or `MarketMakerError::NotIndexed` when the
/// backoff runs out first.
pub async fn confirm_indexed(
    services: &Services,
    signature: Signature,
) -> Result<(), MarketMakerError> {
    let poll = IndexerPollConfig::default();
    let mut confirmed = false;
    for delay in std::iter::once(Duration::ZERO).chain(poll.backoff()) {
        tokio::time::sleep(delay).await;
        if !confirmed {
            confirmed = services
                .rpc
                .confirm_transaction(signature)
                .await
                .map_err(MarketMakerError::Rpc)?;
        }
        if confirmed
            && !services
                .indexer
                .get_shielded_transactions_by_signature(signature, None)
                .await
                .map_err(MarketMakerError::Indexer)?
                .transactions
                .is_empty()
        {
            return Ok(());
        }
    }
    if confirmed {
        Err(MarketMakerError::NotIndexed { signature })
    } else {
        Err(MarketMakerError::NotConfirmed { signature })
    }
}

#[cfg(test)]
mod tests {
    //! Tested invariants:
    //! 1. An aborted step whose operation still has attempts but some of whose
    //!    inputs sync no longer tracks fails with `InputsSpent` counting them,
    //!    also when none of its inputs is tracked any more, instead of being
    //!    requeued.
    //! 2. Exhaustion (`Retry::Fail` or `OPERATION_ATTEMPTS`) wins over every
    //!    other decision; with all inputs tracked the step waits for the spent
    //!    check, and without tracked inputs or after cancellation it is
    //!    requeued.

    use super::*;

    /// Invariant 1: untracked inputs fail the operation with `InputsSpent`,
    /// whether one or all of the step's inputs left the reservations.
    #[test]
    fn untracked_inputs_fail_with_inputs_spent() {
        for (untracked, tracked) in [(1, 1), (2, 0)] {
            let disposal = Disposal::of(DisposalInput {
                retry: Retry::Requeue,
                attempts: 1,
                untracked,
                tracked,
                cancelled: false,
            });
            assert_eq!(
                disposal,
                Disposal::InputsSpent { count: untracked },
                "{untracked} untracked and {tracked} tracked inputs"
            );
        }
    }

    /// Invariant 2: each remaining branch of `discard`'s decision.
    #[test]
    fn disposal_follows_discard_order() {
        let cases = [
            (Retry::Fail, 1, 1, false, 1, Disposal::Exhausted),
            (
                Retry::Requeue,
                OPERATION_ATTEMPTS,
                1,
                false,
                1,
                Disposal::Exhausted,
            ),
            (Retry::Requeue, 1, 0, false, 2, Disposal::AwaitSpentCheck),
            (Retry::Requeue, 1, 0, false, 0, Disposal::Requeue),
            (Retry::Requeue, 1, 0, true, 2, Disposal::Requeue),
        ];
        for (retry, attempts, untracked, cancelled, tracked, want) in cases {
            let got = Disposal::of(DisposalInput {
                retry,
                attempts,
                untracked,
                tracked,
                cancelled,
            });
            assert_eq!(
                got, want,
                "{retry:?}, attempt {attempts}, {untracked} untracked, cancelled {cancelled}, {tracked} tracked"
            );
        }
    }
}

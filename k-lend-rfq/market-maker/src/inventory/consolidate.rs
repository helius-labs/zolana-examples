//! Consolidation: merging and splitting the market maker's own UTXOs of one
//! asset toward its inventory profile. Explicit consolidations are operations;
//! upkeep consolidations run only when the market maker is idle and yield to
//! any fill that needs their UTXOs.

use solana_address::Address;
use solana_signature::Signature;

use crate::{
    api::Inner,
    error::MarketMakerError,
    inventory::balance::{
        profile::InventoryProfile,
        reservations::TrackedUtxo,
        select::{max_outputs_for, select_all, select_smallest, Selection},
    },
    transactions::{
        confirm::Retry,
        coordinator::{Coordinator, Operation, OperationOutcome, ScheduleOutcome},
        steps::{OperationId, StepId, StepKind},
        transfer::{other_amounts, plan_consolidate, TransferStep},
    },
};

/// A landed consolidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsolidateReceipt {
    pub signature: Signature,
    /// UTXOs spent.
    pub inputs: usize,
    /// UTXOs created.
    pub outputs: usize,
}

/// One planned upkeep transfer: spend `selection`, create `parts`.
pub struct Upkeep {
    pub selection: Selection,
    pub parts: Vec<u64>,
}

/// Plans upkeep for one asset against its profile.
pub struct UpkeepPolicy<'a> {
    pub profile: &'a InventoryProfile,
    /// Widest consolidation that fits one transaction.
    pub max_inputs: usize,
}

impl UpkeepPolicy<'_> {
    /// The upkeep transfer for the available `utxos`, or `None` when they
    /// already match the profile. Surplus UTXOs are merged first; only when
    /// there is none is a UTXO split to create missing targets. Errors with
    /// `MarketMakerError::AmountOverflow` when the UTXOs sum above `u64::MAX`.
    pub fn plan(&self, utxos: &[TrackedUtxo]) -> Result<Option<Upkeep>, MarketMakerError> {
        let utxos: Vec<TrackedUtxo> = utxos
            .iter()
            .filter(|utxo| utxo.amount() > 0)
            .cloned()
            .collect();
        let amounts: Vec<u64> = utxos.iter().map(TrackedUtxo::amount).collect();
        let balance = amounts
            .iter()
            .try_fold(0u64, |total, amount| total.checked_add(*amount))
            .ok_or(MarketMakerError::AmountOverflow {
                context: "upkeep balance",
            })?;
        let surplus = self.profile.surplus_utxos(balance, &amounts);
        if surplus > 0 {
            return Ok(self.merge(&utxos, surplus));
        }
        if self.profile.missing(balance, &amounts).is_empty() {
            return Ok(None);
        }
        self.split(&utxos)
    }

    /// Merges the `surplus + 1` smallest UTXOs (capped at `max_inputs`) into
    /// at most `inputs - surplus` parts; `None` when fewer than two inputs
    /// are provable.
    fn merge(&self, utxos: &[TrackedUtxo], surplus: usize) -> Option<Upkeep> {
        let selection = select_smallest(utxos, surplus.saturating_add(1).min(self.max_inputs));
        let inputs = selection.inputs.len();
        if inputs < 2 {
            return None;
        }
        // Only the available UTXOs passed to the planner count, reserved ones
        // are left out.
        let others = other_amounts(
            utxos
                .iter()
                .map(|utxo| (&utxo.wallet.utxo_hash, utxo.amount())),
            &selection.hashes().into_iter().collect(),
        );
        let max_parts = inputs.saturating_sub(surplus).min(max_outputs_for(inputs));
        let parts = self.profile.parts(selection.total, &others, max_parts);
        Some(Upkeep { selection, parts })
    }

    /// Splits the UTXO furthest above its target into a one-input transfer.
    fn split(&self, utxos: &[TrackedUtxo]) -> Result<Option<Upkeep>, MarketMakerError> {
        let amounts: Vec<u64> = utxos.iter().map(TrackedUtxo::amount).collect();
        let Some((utxo_index, parts)) = self.profile.split(&amounts, max_outputs_for(1))? else {
            return Ok(None);
        };
        Ok(utxos
            .get(utxo_index)
            .and_then(Selection::single)
            .map(|selection| Upkeep { selection, parts }))
    }
}

impl Inner {
    /// Consolidates the available UTXOs of `asset` (the largest first, up to
    /// one transaction's width) into the profile's parts and waits for it to
    /// land. Errors with `MarketMakerError::NothingToConsolidate` when fewer
    /// than two UTXOs are available.
    pub async fn consolidate(
        &self,
        asset: Address,
    ) -> Result<ConsolidateReceipt, MarketMakerError> {
        match self.operation(Operation::Consolidate(asset)).await? {
            OperationOutcome::Consolidated(receipt) => Ok(receipt),
            _ => Err(MarketMakerError::UnexpectedOutcome {
                expected: "consolidation",
            }),
        }
    }
}

impl Coordinator {
    /// Schedules the transfer for an explicit consolidation; see
    /// `Inner::consolidate`.
    pub async fn schedule_consolidate(
        &mut self,
        id: OperationId,
        asset: Address,
    ) -> Result<ScheduleOutcome, MarketMakerError> {
        let available = self.services.pending.reservations.available(&asset);
        if available.len() < 2 {
            return Err(MarketMakerError::NothingToConsolidate { asset });
        }
        let selection = select_all(&available, self.services.budget.max_consolidate_inputs);
        let inputs = selection.inputs.len();
        // Every tracked UTXO of the asset outside the selection counts,
        // reserved ones included.
        let tracked = self.services.pending.reservations.utxos(&asset);
        let others = other_amounts(
            tracked.iter().map(|utxo| (&utxo.utxo_hash, utxo.amount)),
            &selection.hashes().into_iter().collect(),
        );
        let parts =
            self.config
                .profile(&asset)
                .parts(selection.total, &others, max_outputs_for(inputs));
        self.schedule_consolidation(asset, Some(id), selection, parts)
            .await
            .map(|_| ScheduleOutcome::Scheduled)
    }

    /// Schedules a consolidation of `selection` of `asset` into `parts`, for
    /// `operation` or, without one, as upkeep.
    async fn schedule_consolidation(
        &mut self,
        asset: Address,
        operation: Option<OperationId>,
        selection: Selection,
        parts: Vec<u64>,
    ) -> Result<StepId, MarketMakerError> {
        let plan = plan_consolidate(selection, 0, parts)?;
        self.schedule_transfer(TransferStep {
            kind: StepKind::Consolidate,
            asset,
            operation,
            plan,
            withdrawal: None,
            tail: None,
            vault_before: None,
            fill: None,
        })
        .await
    }

    /// Schedules one upkeep transfer per asset whose UTXOs drift from its
    /// profile. Runs only when upkeep is enabled (`idle_delay`), nothing is
    /// queued or in flight, and no operation arrived for `idle_delay`; an
    /// asset with unindexed UTXOs is skipped since its balance is not final.
    pub async fn run_upkeep(&mut self) {
        let Some(idle_delay) = self.config.idle_delay else {
            return;
        };
        if !self.is_idle() || self.last_operation.elapsed() < idle_delay {
            return;
        }
        for asset in self.config.assets() {
            if self.services.pending.reservations.unindexed(&asset) {
                continue;
            }
            let policy = UpkeepPolicy {
                profile: self.config.profile(&asset),
                max_inputs: self.services.budget.max_consolidate_inputs,
            };
            let upkeep = match policy.plan(&self.services.pending.reservations.available(&asset)) {
                Ok(Some(upkeep)) => upkeep,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, %asset, "upkeep consolidation could not be planned");
                    continue;
                }
            };
            let Upkeep { selection, parts } = upkeep;
            let scheduled = self
                .schedule_consolidation(asset, None, selection, parts)
                .await;
            if let Err(error) = scheduled {
                tracing::warn!(%error, %asset, "upkeep consolidation could not be scheduled");
            }
        }
    }

    /// Discards every unsent upkeep step on `asset` (requeue, error
    /// `MarketMakerError::Preempted`) so a fill can use its UTXOs. Returns
    /// whether any was discarded. Sent steps are left to land.
    pub async fn preempt_upkeep(&mut self, asset: &Address) -> bool {
        let unsent: Vec<StepId> = self
            .steps
            .in_flight()
            .filter(|step| {
                step.is_upkeep() && step.state.is_unsent() && step.asset.as_ref() == Some(asset)
            })
            .map(|step| step.id)
            .collect();
        for id in &unsent {
            self.discard(*id, MarketMakerError::Preempted, Retry::Requeue)
                .await;
        }
        !unsent.is_empty()
    }
}

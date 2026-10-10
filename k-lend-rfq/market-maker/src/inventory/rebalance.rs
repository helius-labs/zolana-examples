//! Rebalancing the market maker's inventory through the vault: a public kVault
//! deposit or withdrawal funded by an unshielding transfer and followed by a
//! shield of the proceeds, in one transaction. Automatic rebalances keep each
//! asset inside its target range and never push the other asset out of its
//! own.
//!
//! The coordinator loop never waits on rpc for a rebalance. The vault read,
//! the sizing and the tail preview run off the loop, in a task spawned by
//! `Coordinator::check_ranges` (automatic) or in the api caller's task
//! (`Inner::rebalance`, manual), and the queued [`RebalanceOrder`] carries
//! the preview, so `Coordinator::schedule_rebalance` only selects inputs.
//! Each rebalance is a public on-chain operation, so none is retried
//! blindly: an automatic rebalance that leaves the balances unchanged is
//! triggered again only after a growing backoff.

use std::time::{Duration, Instant};

use solana_address::Address;
use solana_instruction::Instruction;
use solana_signature::Signature;
use tokio::sync::oneshot;
use zolana_client::AsyncRpc;
use zolana_interface::pda;

use k_lend_rfq_sdk::{
    kvault::{
        deposit_instruction, withdraw_from_available_instruction, withdraw_instruction,
        KvaultError, UserAccounts,
    },
    pair::{Pair, VaultError, VaultState, WithdrawSource},
    transfer::smallest_shape,
};

use crate::{
    api::{Holdings, Inner, VaultOperation},
    config::TargetRange,
    error::MarketMakerError,
    inventory::balance::select::{max_outputs_for, select},
    transactions::{
        confirm::confirm_indexed,
        coordinator::{
            Coordinator, Event, Operation, OperationOutcome, QueuedOperation, ScheduleOutcome,
        },
        kvault::{read_vault, token_balance},
        shield::{vault_compute_units, ShieldPlan},
        steps::{OperationId, StepKind, TailShield},
        transfer::{other_amounts, plan_consolidate, TransferStep, WithdrawalTarget},
        Identity,
    },
};

/// The longest an automatic rebalance waits before it is triggered again in
/// an unchanged situation (`RebalanceBackoff`).
const REBALANCE_BACKOFF_CAP: Duration = Duration::from_secs(300);

/// What a rebalance acquires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebalanceKind {
    /// A vault deposit: collateral spent, shares received.
    Shares,
    /// A vault withdrawal: shares spent, collateral received.
    Collateral,
}

impl RebalanceKind {
    /// The asset of `pair` the market maker unshields and hands to the vault.
    pub fn spent_asset(self, pair: &Pair) -> Address {
        match self {
            Self::Shares => pair.token_mint,
            Self::Collateral => pair.shares_mint,
        }
    }

    /// The asset of `pair` the vault pays and the market maker shields.
    pub fn received_asset(self, pair: &Pair) -> Address {
        match self {
            Self::Shares => pair.shares_mint,
            Self::Collateral => pair.token_mint,
        }
    }
}

/// A rebalance through `pair` spending up to `amount` of the spent asset,
/// before its vault preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebalanceRequest {
    pub pair: Pair,
    pub kind: RebalanceKind,
    pub amount: u64,
}

/// A previewed rebalance, carried by `Operation::Rebalance`. The preview
/// fixes the amount the rebalance spends (`tail.withdrawal`) and the shape
/// of its transaction, so scheduling it needs no rpc. A requeued attempt
/// reuses the preview: the shield amount is resolved from a simulation
/// right before sending (`TailShield::resolve`) and never exceeds what the
/// account holds, whatever changed since the preview.
pub struct RebalanceOrder {
    pub pair: Pair,
    pub kind: RebalanceKind,
    pub tail: Box<RebalanceTail>,
}

impl RebalanceOrder {
    /// The asset the market maker unshields and hands to the vault.
    pub fn spent_asset(&self) -> Address {
        self.kind.spent_asset(&self.pair)
    }

    /// The amount of the spent asset the rebalance pays to the market maker's
    /// public account (`RebalanceTail::withdrawal`).
    pub fn amount(&self) -> u64 {
        self.tail.withdrawal
    }
}

/// The balances and ranges of a pair whose assets are out of range while no
/// rebalance through it fits both ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeConflict {
    /// The pair's net balances (`PendingBalance::net_balance`).
    pub balances: Holdings,
    pub collateral_range: Option<TargetRange>,
    pub shares_range: Option<TargetRange>,
}

/// A rebalance through a pair that the vault preview refuses: the vault
/// would fail the kVault instruction with `error` (a deposit cap reached, an
/// amount below the vault minimum, or a withdrawal above the liquidity one
/// instruction can pay out).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebalanceSkip {
    pub kind: RebalanceKind,
    pub amount: u64,
    pub error: VaultError,
}

/// What `check_ranges` last warned about for a vault, so it warns once per
/// change rather than every tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeWarning {
    Conflict(RangeConflict),
    Skip(RebalanceSkip),
}

/// What `rebalance_need` found for a pair.
#[derive(Debug, PartialEq, Eq)]
enum RebalanceNeed {
    InRange,
    Order(RebalanceRequest),
    Conflict(RangeConflict),
    Skip(RebalanceSkip),
}

/// The situation an automatic rebalance was triggered in: the pair's net
/// balances and the vault's uninvested tokens and issued shares. A rebalance
/// that lands changes the balances, so the same key again means the earlier one
/// failed (or the vault and the market maker were otherwise left as they were).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackoffKey {
    balances: Holdings,
    token_available: u64,
    shares_issued: u64,
}

/// Per vault, the last automatic rebalance and when the same situation may
/// trigger one again (`Coordinator::check_ranges`).
#[derive(Clone, Copy, Debug)]
pub struct RebalanceBackoff {
    key: BackoffKey,
    /// Rebalances triggered in a row under `key`.
    triggers: u32,
    until: Instant,
}

/// The wait after the `triggers`-th rebalance in a row under one key:
/// `base * 2^(triggers - 1)`, at most `REBALANCE_BACKOFF_CAP`.
fn backoff_delay(base: Duration, triggers: u32) -> Duration {
    1u32.checked_shl(triggers.saturating_sub(1))
        .and_then(|factor| base.checked_mul(factor))
        .map_or(REBALANCE_BACKOFF_CAP, |delay| {
            delay.min(REBALANCE_BACKOFF_CAP)
        })
}

/// A pair `check_ranges` found out of range, with everything the preview
/// task needs besides the rpc and the identity.
pub struct RangeCandidate {
    pair: Pair,
    balances: Holdings,
    collateral_range: Option<TargetRange>,
    shares_range: Option<TargetRange>,
    /// Splits the collateral a withdrawal pays.
    collateral_plan: ShieldPlan,
    /// Splits the shares a deposit mints.
    shares_plan: ShieldPlan,
}

/// What the preview task found for one pair.
pub enum RangeOutcome {
    InRange,
    Order {
        order: RebalanceOrder,
        key: BackoffKey,
    },
    Conflict(RangeConflict),
    Skip(RebalanceSkip),
}

/// One pair the preview task checked, with the balances it was checked at.
pub struct CheckedPair {
    pub pair: Pair,
    pub balances: Holdings,
    pub outcome: Result<RangeOutcome, MarketMakerError>,
}

/// The vault preview errors that block a rebalance until the vault or the
/// market maker's balances change, rather than failing the range check:
/// retrying them every tick would only repeat the warning.
fn blocking(error: &anyhow::Error) -> Option<VaultError> {
    error
        .downcast_ref::<VaultError>()
        .filter(|error| {
            matches!(
                error,
                VaultError::DepositCapReached { .. }
                    | VaultError::BelowMinimumDeposit { .. }
                    | VaultError::BelowMinimumWithdraw { .. }
                    | VaultError::WithdrawExceedsLiquidity { .. }
            )
        })
        .cloned()
}

/// The vault side of a rebalance transaction, built from a vault preview.
pub struct RebalanceTail {
    /// The vault state the preview ran against.
    pub before: VaultState,
    /// Amount of the spent asset the transfer pays to the market maker's public
    /// account: the deposit's tokens (crank funds included) or the shares
    /// the withdrawal burns.
    pub withdrawal: u64,
    /// The kVault instruction and the shield, resolved before sending.
    pub shield: TailShield,
    /// The widest tail the shield can resolve to
    /// (`TailShield::sizing_instructions`), used to size the transaction.
    pub sizing: Vec<Instruction>,
}

impl RebalanceRequest {
    /// The tail of the request against vault state `before`.
    /// `account_before` is the balance of the market maker's account of the
    /// received asset, swept by the shield; `plan` splits the shielded
    /// amount, and `also_shield` adds UTXOs of other assets to the shield
    /// instruction. The shielded amount is resolved by `TailShield::resolve`;
    /// the transaction is sized with a shield of `plan.max_utxos` parts, the
    /// most the resolved amount can be split into.
    pub fn tail(
        &self,
        before: VaultState,
        market_maker: &Identity,
        plan: ShieldPlan,
        account_before: u64,
        also_shield: Vec<(Address, u64)>,
    ) -> Result<RebalanceTail, MarketMakerError> {
        let pair = &self.pair;
        let user = UserAccounts::associated(market_maker.payer, pair);
        let math = |error: anyhow::Error| MarketMakerError::VaultMath {
            vault: pair.vault,
            reason: error.to_string(),
        };
        let instruction = |error: KvaultError| MarketMakerError::VaultInstruction {
            vault: pair.vault,
            reason: error.to_string(),
        };
        let (withdrawal, vault_instruction) = match self.kind {
            RebalanceKind::Shares => {
                let outcome = before.deposit(self.amount).map_err(math)?;
                let deposit = deposit_instruction(&pair.vault, &before, &user, outcome.tokens)
                    .map_err(instruction)?;
                (outcome.tokens, deposit)
            }
            RebalanceKind::Collateral => {
                let outcome = before.withdraw(self.amount).map_err(math)?;
                let source = before
                    .withdraw_source(outcome.tokens)
                    .map_err(|error| math(error.into()))?;
                let withdraw = match source {
                    WithdrawSource::Available => withdraw_from_available_instruction(
                        &pair.vault,
                        &before,
                        &user,
                        outcome.shares,
                    ),
                    WithdrawSource::Reserve(reserve) => {
                        withdraw_instruction(&pair.vault, &before, &user, &reserve, outcome.shares)
                    }
                }
                .map_err(instruction)?;
                (outcome.shares, withdraw)
            }
        };
        let asset = self.kind.received_asset(pair);
        let shield = TailShield {
            vault_instruction,
            asset,
            asset_account: pda::associated_token_address(&market_maker.payer, &asset),
            before: account_before,
            plan,
            also_shield,
            compute_units: vault_compute_units(before.reserves.as_slice().len()),
        };
        let sizing = shield.sizing_instructions(market_maker)?;
        Ok(RebalanceTail {
            before,
            withdrawal,
            shield,
            sizing,
        })
    }
}

/// Previews `request` against `state`: reads the balance of the market maker's
/// account of the received asset (swept by the shield) and builds the tail.
/// Errors with the rpc error, `MarketMakerError::TokenAccount`, or the errors
/// of `RebalanceRequest::tail`.
async fn preview(
    rpc: &dyn AsyncRpc,
    identity: &Identity,
    request: RebalanceRequest,
    state: VaultState,
    plan: ShieldPlan,
) -> Result<RebalanceOrder, MarketMakerError> {
    let account_before = token_balance(
        rpc,
        pda::associated_token_address(&identity.payer, &request.kind.received_asset(&request.pair)),
    )
    .await?;
    let tail = request.tail(state, identity, plan, account_before, Vec::new())?;
    Ok(RebalanceOrder {
        pair: request.pair,
        kind: request.kind,
        tail: Box::new(tail),
    })
}

impl Inner {
    /// Runs one rebalance of `amount` of the spent asset now and waits until
    /// it is indexed. Syncs first so the selection sees the latest inventory,
    /// then reads the vault and previews the rebalance in this task, off the
    /// coordinator loop. Errors with the rpc and preview errors, as
    /// `Coordinator::schedule_rebalance` rejects, or with the send and
    /// confirm errors.
    pub async fn rebalance(
        &self,
        pair: &Pair,
        kind: RebalanceKind,
        amount: u64,
    ) -> Result<VaultOperation, MarketMakerError> {
        self.sync().await?;
        let rpc = self.services.rpc.as_ref();
        let state = read_vault(rpc, pair.vault).await?;
        let plan = ShieldPlan::new(
            &self.settings(),
            &self.services.pending.reservations,
            &kind.received_asset(pair),
        );
        let request = RebalanceRequest {
            pair: *pair,
            kind,
            amount,
        };
        let order = preview(rpc, &self.identity, request, state, plan).await?;
        match self.operation(Operation::Rebalance(order)).await? {
            OperationOutcome::Rebalanced {
                receipt,
                vault_before,
            } => {
                self.settled(pair, *vault_before, receipt.signature, receipt.inputs)
                    .await
            }
            _ => Err(MarketMakerError::UnexpectedOutcome {
                expected: "rebalance",
            }),
        }
    }

    /// Waits until `signature` is indexed, syncs, and reports the vault's
    /// change from `before` to its current state; `tokens` and `shares` are
    /// the changes of `token_available` and `shares_issued`.
    pub async fn settled(
        &self,
        pair: &Pair,
        before: VaultState,
        signature: Signature,
        inputs: usize,
    ) -> Result<VaultOperation, MarketMakerError> {
        confirm_indexed(&self.services, signature).await?;
        self.sync().await?;
        let after = read_vault(self.services.rpc.as_ref(), pair.vault).await?;
        Ok(VaultOperation {
            tokens: before.token_available.abs_diff(after.token_available),
            shares: before.shares_issued.abs_diff(after.shares_issued),
            before,
            after,
            inputs,
            signature,
        })
    }
}

impl Coordinator {
    /// Schedules the transfer that pays `order.amount()` of the spent asset to
    /// the market maker's public account, followed by the kVault instruction
    /// and the shield of the received asset, both from the order's preview. No
    /// rpc runs here: the shield amount is resolved from a simulation in the
    /// prove task (`TailShield::resolve`), and the transaction is sized with
    /// the widest shield that resolution can produce.
    ///
    /// A rebalance reserves exactly the UTXOs it spends: inputs are chosen
    /// with `select`, the fewest UTXOs covering the amount within the
    /// budget, as fills choose theirs. Every other UTXO of the asset stays
    /// available, so concurrent fills on the same asset keep their inventory.
    /// The change outputs of the rebalance return to the pool when it lands.
    /// If no selection covers the amount, the operation is `Backlogged` while
    /// UTXOs of the asset are in flight or unindexed, and is otherwise
    /// rejected with `InsufficientBalance` (the selectable balance is short)
    /// or `FragmentedInventory` (the balance is spread over too many UTXOs).
    pub async fn schedule_rebalance(
        &mut self,
        id: OperationId,
        order: &RebalanceOrder,
    ) -> Result<ScheduleOutcome, MarketMakerError> {
        // 1. The preview fixes the amount and the tail's shape.
        let asset = order.spent_asset();
        let tail = &order.tail;
        let withdrawal_target = WithdrawalTarget {
            owner: self.identity.payer,
            token_program: pda::spl_token_program_id(),
        };
        // 2. The widest input count that fits next to the public withdrawal
        //    and the tail.
        let accounts = withdrawal_target.spl_accounts(asset);
        let budget = self.services.budget.clone();
        let max_inputs = budget.max_consolidate_inputs_with(1, accounts, &tail.sizing)?;
        // 3. The fewest available UTXOs covering the withdrawal.
        let available = self.services.pending.reservations.available(&asset);
        let Some(selection) = select(&available, tail.withdrawal, max_inputs) else {
            if self.waits_for_utxos(&asset) {
                return Ok(ScheduleOutcome::Backlogged);
            }
            let total = available
                .iter()
                .filter(|utxo| utxo.leaf_index.is_some())
                .try_fold(0u64, |total, utxo| total.checked_add(utxo.amount()))
                .ok_or(MarketMakerError::AmountOverflow {
                    context: "rebalance selectable balance",
                })?;
            return Err(if total < tail.withdrawal {
                MarketMakerError::InsufficientBalance {
                    asset,
                    available: total,
                    requested: tail.withdrawal,
                }
            } else {
                MarketMakerError::FragmentedInventory {
                    asset,
                    available: self.services.pending.reservations.balance(&asset),
                    requested: tail.withdrawal,
                    max_inputs,
                }
            });
        };
        // 4. Split the change toward the profile with the most outputs whose
        //    shape still fits the transaction.
        let inputs = selection.inputs.len();
        let kept = selection.total.checked_sub(tail.withdrawal).ok_or(
            MarketMakerError::AmountOverflow {
                context: "rebalance change",
            },
        )?;
        // Every tracked UTXO of the asset outside the selection counts,
        // reserved ones included.
        let tracked = self.services.pending.reservations.utxos(&asset);
        let others = other_amounts(
            tracked.iter().map(|utxo| (&utxo.utxo_hash, utxo.amount)),
            &selection.hashes().into_iter().collect(),
        );
        let profile = self.config.profile(&asset);
        let parts = (1..=max_outputs_for(inputs))
            .rev()
            .map(|max_parts| profile.parts(kept, &others, max_parts))
            .find(|parts| {
                smallest_shape(inputs, parts.len().max(1)).is_some_and(|shape| {
                    budget
                        .consolidate_size(shape, accounts, &tail.sizing)
                        .is_ok_and(|size| size.fits())
                })
            })
            .unwrap_or_else(|| profile.parts(kept, &others, 1));
        let plan = plan_consolidate(selection, tail.withdrawal, parts)?;
        self.schedule_transfer(TransferStep {
            kind: StepKind::Rebalance,
            asset,
            operation: Some(id),
            plan,
            withdrawal: Some(withdrawal_target),
            tail: Some(tail.shield.clone()),
            vault_before: Some(tail.before.clone()),
            fill: None,
        })
        .await
        .map(|_| ScheduleOutcome::Scheduled)
    }

    /// Starts the automatic range check when the market maker is idle (nothing
    /// queued, no step in flight, no range preview running). Runs no rpc:
    /// it only classifies balances and spawns the preview.
    ///
    /// Balances are the net balances (`PendingBalance::net_balance`), so a
    /// user's payment that has not landed yet counts. Ranges are per asset,
    /// not per pair. The pairs are walked in configuration order only to
    /// find a vault to rebalance through: a pair is a candidate when it is
    /// not retiring, both its assets are indexed, one of them is out of
    /// range, and it is not in backoff (below). A pair found in range has its
    /// warning and backoff cleared here.
    ///
    /// The candidates go to one spawned task that, per candidate in order,
    /// reads the vault, sizes the rebalance (`rebalance_need`) and previews
    /// its tail, stopping at the first order, and reports through
    /// `Event::RangesPreviewed`; `on_ranges_previewed` logs and queues. So
    /// two pairs cannot both act on one surplus in a tick.
    ///
    /// Backoff: when a rebalance is triggered, the vault records its key
    /// (`BackoffKey`: the pair's balances and the vault's `token_available`
    /// and `shares_issued`) and a deadline. Triggering again under the same
    /// key doubles the wait: `base * 2^(n - 1)` after the n-th trigger in a
    /// row, capped at five minutes (`REBALANCE_BACKOFF_CAP`), where `base`
    /// is the upkeep delay (`utxo_upkeep_delay`) or, with upkeep disabled,
    /// the status interval. Until the deadline a pair whose balances are
    /// unchanged is not even previewed. A rebalance that lands changes the
    /// balances and so the key, so only a failing (or otherwise ineffective)
    /// rebalance waits; an unmodelled kVault error costs one proof and one
    /// fee per backoff period, not per tick.
    pub fn check_ranges(&mut self) {
        if !self.is_idle() || self.preview_in_flight {
            return;
        }
        let now = Instant::now();
        let mut candidates = Vec::new();
        for pair in &self.config.pairs {
            if self.config.retiring.contains(&pair.vault) {
                continue;
            }
            let reservations = &self.services.pending.reservations;
            if reservations.unindexed(&pair.token_mint) || reservations.unindexed(&pair.shares_mint)
            {
                continue;
            }
            let balances = self.pair_balances(pair);
            let collateral_range = self.config.range(&pair.token_mint);
            let shares_range = self.config.range(&pair.shares_mint);
            let in_range = |range: Option<TargetRange>, balance| {
                range.is_none_or(|range| range.contains(balance))
            };
            if in_range(collateral_range, balances.collateral)
                && in_range(shares_range, balances.shares)
            {
                self.range_warnings.remove(&pair.vault);
                self.rebalance_backoff.remove(&pair.vault);
                continue;
            }
            let waiting = self
                .rebalance_backoff
                .get(&pair.vault)
                .is_some_and(|backoff| backoff.key.balances == balances && now < backoff.until);
            if waiting {
                continue;
            }
            candidates.push(RangeCandidate {
                pair: *pair,
                balances,
                collateral_range,
                shares_range,
                collateral_plan: ShieldPlan::new(&self.config, reservations, &pair.token_mint),
                shares_plan: ShieldPlan::new(&self.config, reservations, &pair.shares_mint),
            });
        }
        if candidates.is_empty() {
            return;
        }
        self.preview_in_flight = true;
        let rpc = self.services.rpc.clone();
        let identity = self.identity.clone();
        let events = self.runtime.events.clone();
        let cancel = self.runtime.cancel.clone();
        self.runtime.tasks.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                checked = preview_ranges(rpc.as_ref(), &identity, candidates) => {
                    // A closed channel means the coordinator stopped.
                    let _ = events.send(Event::RangesPreviewed(checked));
                }
            }
        });
    }

    /// Applies a range preview (`check_ranges`). Per checked pair: `InRange`
    /// clears its warning; `Conflict` (out of range, but no rebalance fits
    /// both ranges) and `Skip` (the vault preview refuses the rebalance) are
    /// logged at `warn` with the balances and ranges or the vault error, once
    /// per change of those values (kept in `range_warnings`), not every
    /// tick; an error (a failed vault read, say) is logged every time.
    /// Every `Conflict` and `Skip` is counted (`MarketMaker::range_refusals`),
    /// logged or not.
    ///
    /// An order is queued only if the preview still applies: the market maker
    /// is still idle, the pair is still served unchanged, and both net balances
    /// equal the ones the preview sized it with. Otherwise it is dropped and
    /// the next idle tick checks again.
    pub async fn on_ranges_previewed(&mut self, checked: Vec<CheckedPair>) {
        self.preview_in_flight = false;
        for CheckedPair {
            pair,
            balances,
            outcome,
        } in checked
        {
            match outcome {
                Ok(RangeOutcome::InRange) => {
                    self.range_warnings.remove(&pair.vault);
                }
                Ok(RangeOutcome::Conflict(conflict)) => {
                    self.services.pending.record_range_refusal();
                    self.warn_range(pair.vault, RangeWarning::Conflict(conflict));
                }
                Ok(RangeOutcome::Skip(skip)) => {
                    self.services.pending.record_range_refusal();
                    self.warn_range(pair.vault, RangeWarning::Skip(skip));
                }
                Ok(RangeOutcome::Order { order, key }) if self.preview_applies(&pair, balances) => {
                    self.range_warnings.remove(&pair.vault);
                    self.trigger_rebalance(order, key).await;
                }
                Ok(RangeOutcome::Order { .. }) => tracing::debug!(
                    vault = %pair.vault,
                    "rebalance preview is stale; the next idle tick checks again"
                ),
                Err(error) => {
                    tracing::warn!(vault = %pair.vault, %error, "target range check failed")
                }
            }
        }
    }

    /// Logs `warning` for `vault` at `warn` unless it equals the last one
    /// logged for the vault, and records it.
    fn warn_range(&mut self, vault: Address, warning: RangeWarning) {
        if self.range_warnings.get(&vault) == Some(&warning) {
            return;
        }
        match &warning {
            RangeWarning::Conflict(conflict) => tracing::warn!(
                vault = %vault,
                collateral = conflict.balances.collateral,
                shares = conflict.balances.shares,
                collateral_range = ?conflict.collateral_range,
                shares_range = ?conflict.shares_range,
                "out of target range, but no rebalance fits both ranges"
            ),
            RangeWarning::Skip(skip) => tracing::warn!(
                vault = %vault,
                kind = ?skip.kind,
                amount = skip.amount,
                error = %skip.error,
                "out of target range, but the vault refuses the rebalance"
            ),
        }
        self.range_warnings.insert(vault, warning);
    }

    /// Whether an order previewed for `pair` at `balances` still applies: the
    /// market maker is idle, the pair is served unchanged and not retiring, and
    /// its net balances are still `balances`.
    fn preview_applies(&self, pair: &Pair, balances: Holdings) -> bool {
        self.is_idle()
            && self.config.pair(&pair.vault) == Some(pair)
            && !self.config.retiring.contains(&pair.vault)
            && self.pair_balances(pair) == balances
    }

    /// The pair's net balances (`PendingBalance::net_balance`).
    fn pair_balances(&self, pair: &Pair) -> Holdings {
        let pending = &self.services.pending;
        Holdings {
            collateral: pending.net_balance(&pair.token_mint),
            shares: pending.net_balance(&pair.shares_mint),
        }
    }

    /// Queues the previewed `order`, after recording its backoff under `key`
    /// (see `check_ranges`). The order spends `order.amount()`, the amount
    /// its tail fixed.
    async fn trigger_rebalance(&mut self, order: RebalanceOrder, key: BackoffKey) {
        let vault = order.pair.vault;
        let triggers = match self.rebalance_backoff.get(&vault) {
            Some(backoff) if backoff.key == key => backoff.triggers.saturating_add(1),
            _ => 1,
        };
        let base = self
            .config
            .idle_delay
            .unwrap_or(self.config.status_interval);
        let now = Instant::now();
        self.rebalance_backoff.insert(
            vault,
            RebalanceBackoff {
                key,
                triggers,
                // `REBALANCE_BACKOFF_CAP` from now always fits an `Instant`.
                until: now
                    .checked_add(backoff_delay(base, triggers))
                    .unwrap_or(now),
            },
        );
        let asset = order.spent_asset();
        if let Err(error) = self.services.pending.queue(asset, order.amount()) {
            tracing::warn!(%error, "automatic rebalance cannot be queued");
            return;
        }
        let (reply, outcome) = oneshot::channel();
        self.queue.push_back(QueuedOperation {
            id: self.services.pending.next_operation(),
            operation: Operation::Rebalance(order),
            reply,
            attempts: 0,
        });
        self.services.pending.record_triggered_rebalance();
        let pending = self.services.pending.clone();
        self.runtime.tasks.spawn(async move {
            match outcome.await {
                Ok(Ok(OperationOutcome::Rebalanced { receipt, .. })) => {
                    pending.record_rebalance(receipt.signature);
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(%error, "automatic rebalance failed"),
                Err(_) => {}
            }
        });
        self.try_schedule().await;
    }
}

/// The preview task of `check_ranges`: checks `candidates` in order and
/// stops after the first one that yields an order.
async fn preview_ranges(
    rpc: &dyn AsyncRpc,
    identity: &Identity,
    candidates: Vec<RangeCandidate>,
) -> Vec<CheckedPair> {
    let mut checked = Vec::new();
    for candidate in candidates {
        let (pair, balances) = (candidate.pair, candidate.balances);
        let outcome = candidate.preview(rpc, identity).await;
        let ordered = matches!(outcome, Ok(RangeOutcome::Order { .. }));
        checked.push(CheckedPair {
            pair,
            balances,
            outcome,
        });
        if ordered {
            break;
        }
    }
    checked
}

impl RangeCandidate {
    /// Reads the vault, sizes the rebalance with `rebalance_need` and, for
    /// an order, previews its tail. Errors with the rpc and preview errors.
    async fn preview(
        self,
        rpc: &dyn AsyncRpc,
        identity: &Identity,
    ) -> Result<RangeOutcome, MarketMakerError> {
        let state = read_vault(rpc, self.pair.vault).await?;
        let key = BackoffKey {
            balances: self.balances,
            token_available: state.token_available,
            shares_issued: state.shares_issued,
        };
        let request = match rebalance_need(
            &self.pair,
            self.balances,
            self.collateral_range,
            self.shares_range,
            &state,
        )? {
            RebalanceNeed::InRange => return Ok(RangeOutcome::InRange),
            RebalanceNeed::Conflict(conflict) => return Ok(RangeOutcome::Conflict(conflict)),
            RebalanceNeed::Skip(skip) => return Ok(RangeOutcome::Skip(skip)),
            RebalanceNeed::Order(request) => request,
        };
        let plan = match request.kind {
            RebalanceKind::Shares => self.shares_plan,
            RebalanceKind::Collateral => self.collateral_plan,
        };
        let order = preview(rpc, identity, request, state, plan).await?;
        Ok(RangeOutcome::Order { order, key })
    }
}

/// The rebalance through `pair` that moves its two assets toward their
/// target ranges, from the net `balances` and the vault `state`: `InRange`
/// if both are in range (or have none), `Conflict` if one is out of range
/// but no amount fits, `Skip` if the vault preview refuses the amount with a
/// `blocking` error (deposit cap reached, below the vault's minimum deposit
/// or withdrawal, above the liquidity one withdraw instruction can pay out).
///
/// Invariant: a rebalance never pushes the asset it receives above that
/// asset's own `max`, and never pulls the asset it spends below that
/// asset's `min`. Ranges that are incompatible with the vault rate (both
/// assets above `max`, say) produce no order rather than a deposit followed
/// by a withdraw every idle tick, each a public kVault operation.
///
/// - Deposit (collateral spent, shares received), when collateral is above
///   its max or shares are below their min: moves collateral down to the
///   middle of its range, or enough to lift shares to the middle of theirs
///   while keeping collateral at or above its min; capped so the minted
///   shares keep shares at or below their max (vault preview).
/// - Withdraw (shares spent, collateral received), tried when no deposit is
///   possible and shares are above their max or collateral below its min:
///   symmetric, capped so the redeemed collateral keeps collateral at or
///   below its max, and the burned shares at most the vault's issued shares.
///
/// Missing shares are converted to collateral with the pure AUM ratio
/// (`VaultState::tokens_for_shares`) and missing collateral to shares with
/// the plain share math (`VaultState::shares_for_deposit`): the conversion
/// only sizes the target, and the vault's limits apply in the capping step.
///
/// When neither fits, a `blocking` refusal of the deposit, else of the
/// withdrawal, yields `Skip`; otherwise the result is `Conflict`. Errors of
/// the conversions return `MarketMakerError::VaultMath`.
fn rebalance_need(
    pair: &Pair,
    balances: Holdings,
    collateral_range: Option<TargetRange>,
    shares_range: Option<TargetRange>,
    state: &VaultState,
) -> Result<RebalanceNeed, MarketMakerError> {
    // 1. Classify both assets against their ranges.
    let Holdings { collateral, shares } = balances;
    let too_much_collateral = collateral_range.filter(|range| collateral > range.max());
    let too_few_shares = shares_range.filter(|range| shares < range.min());
    let too_little_collateral = collateral_range.filter(|range| collateral < range.min());
    let too_many_shares = shares_range.filter(|range| shares > range.max());
    let any = too_much_collateral.is_some()
        || too_few_shares.is_some()
        || too_little_collateral.is_some()
        || too_many_shares.is_some();
    if !any {
        return Ok(RebalanceNeed::InRange);
    }
    let order = |kind, amount| RebalanceRequest {
        pair: *pair,
        kind,
        amount,
    };
    let math = |error: VaultError| MarketMakerError::VaultMath {
        vault: pair.vault,
        reason: error.to_string(),
    };
    // 2. The deposit that would fix the out-of-range side, in collateral.
    let collateral_floor = collateral_range.map_or(0, |range| range.min());
    let shares_floor = shares_range.map_or(0, |range| range.min());
    let deposit = match (too_much_collateral, too_few_shares) {
        (Some(range), _) => Some(collateral.saturating_sub(range.middle())),
        (None, Some(range)) => {
            let missing = range.middle().saturating_sub(shares);
            let tokens = state.tokens_for_shares(missing).map_err(math)?;
            Some(tokens.min(collateral.saturating_sub(collateral_floor)))
        }
        (None, None) => None,
    };
    // 3. Cap the deposit so the minted shares stay at or below the share
    //    max; any positive amount left is the order.
    let shares_room = shares_range.map_or(u64::MAX, |range| range.max().saturating_sub(shares));
    let mut refused = None;
    if let Some(amount) = deposit.filter(|amount| *amount > 0) {
        let capped = deposit_cap(state, amount, shares_room);
        if capped > 0 {
            return Ok(RebalanceNeed::Order(order(RebalanceKind::Shares, capped)));
        }
        refused = state
            .deposit(amount)
            .err()
            .as_ref()
            .and_then(blocking)
            .map(|error| (RebalanceKind::Shares, amount, error));
    }
    // 4. Otherwise the withdrawal, in shares, symmetric to step 2.
    let withdrawal = match (too_many_shares, too_little_collateral) {
        (Some(range), _) => Some(shares.saturating_sub(range.middle())),
        (None, Some(range)) => {
            let missing = range.middle().saturating_sub(collateral);
            let needed = state.shares_for_deposit(missing).map_err(math)?;
            Some(needed.min(shares.saturating_sub(shares_floor)))
        }
        (None, None) => None,
    };
    // 5. Cap it so the redeemed collateral stays at or below its max.
    let collateral_room =
        collateral_range.map_or(u64::MAX, |range| range.max().saturating_sub(collateral));
    if let Some(amount) = withdrawal.filter(|amount| *amount > 0) {
        let capped = withdraw_cap(state, amount, collateral_room);
        if capped > 0 {
            return Ok(RebalanceNeed::Order(order(
                RebalanceKind::Collateral,
                capped,
            )));
        }
        refused = refused.or_else(|| {
            state
                .withdraw(amount.min(state.shares_issued))
                .err()
                .as_ref()
                .and_then(blocking)
                .map(|error| (RebalanceKind::Collateral, amount, error))
        });
    }
    // 6. Nothing fits: blame the vault if it refused, else the ranges.
    if let Some((kind, amount, error)) = refused {
        return Ok(RebalanceNeed::Skip(RebalanceSkip {
            kind,
            amount,
            error,
        }));
    }
    Ok(RebalanceNeed::Conflict(RangeConflict {
        balances,
        collateral_range,
        shares_range,
    }))
}

/// The largest collateral amount up to `amount` whose deposit mints at most
/// `room` shares (vault preview), or 0 if that amount mints no share.
fn deposit_cap(state: &VaultState, amount: u64, room: u64) -> u64 {
    let capped = largest_fitting(amount, |tokens| {
        state
            .deposit(tokens)
            .map_or(true, |outcome| outcome.shares <= room)
    });
    match state.deposit(capped) {
        Ok(outcome) if outcome.shares > 0 => capped,
        _ => 0,
    }
}

/// The largest share amount up to `amount`, and up to the vault's issued
/// shares, whose withdrawal pays at most `room` collateral and at most what
/// `token_available` plus one reserve cover (vault preview,
/// `WithdrawExceedsLiquidity`), or 0 if that amount pays nothing.
fn withdraw_cap(state: &VaultState, amount: u64, room: u64) -> u64 {
    let capped = largest_fitting(amount, |shares| {
        shares <= state.shares_issued
            && match state.withdraw(shares) {
                Ok(outcome) => outcome.tokens <= room,
                // Pays more than one instruction can draw (`withdraw_source`).
                Err(error) => !matches!(
                    error.downcast_ref::<VaultError>(),
                    Some(VaultError::WithdrawExceedsLiquidity { .. })
                ),
            }
    });
    match state.withdraw(capped) {
        Ok(outcome) if outcome.tokens > 0 => capped,
        _ => 0,
    }
}

/// The largest value in `0..=upper` for which `fits` holds, where `fits`
/// holds on a prefix of `0..=upper` (the vault previews are monotonic in the
/// amount) including 0.
fn largest_fitting(upper: u64, fits: impl Fn(u64) -> bool) -> u64 {
    if fits(upper) {
        return upper;
    }
    // `fits(low)` holds and `fits(high)` does not.
    let (mut low, mut high) = (0u64, upper);
    while high.saturating_sub(low) > 1 {
        let middle = low.saturating_add(high.saturating_sub(low) / 2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

#[cfg(test)]
mod tests {
    use k_lend_rfq_sdk::pair::Reserves;

    use super::*;

    /// 3 tokens per 2 shares: deposits and withdrawals round in the vault's
    /// favour.
    const STATE: VaultState = VaultState {
        token_mint: Address::new_from_array([1; 32]),
        token_program: Address::new_from_array([2; 32]),
        token_available: 3_000,
        shares_issued: 2_000,
        pending_fees_sf: 0,
        reserves: Reserves::EMPTY,
        min_deposit_amount: 0,
        min_withdraw_amount: 0,
        deposit_cap: 0,
        crank_funds: 0,
        withdrawal_penalty_lamports: 0,
        withdrawal_penalty_bps: 0,
        global_withdrawal_penalty_lamports: 0,
        global_withdrawal_penalty_bps: 0,
    };

    /// `blocking` returns the vault limits a rebalance waits out (deposit
    /// cap, minimums, liquidity) and `None` for every other error.
    #[test]
    fn blocking_returns_vault_limits_only() {
        // Below the minimum: 10 tokens mint 6 shares worth 9 tokens.
        let minimum = VaultState {
            min_deposit_amount: 100,
            ..STATE
        };
        let below_minimum = minimum
            .deposit(10)
            .expect_err("a deposit of 10 below a minimum of 100 fails");
        let cap = VaultError::DepositCapReached { cap: 10, aum: 10 };
        let liquidity = VaultError::WithdrawExceedsLiquidity {
            tokens: 2,
            available: 1,
        };
        for (label, error, want) in [
            ("deposit cap", anyhow::Error::new(cap.clone()), Some(cap)),
            (
                "withdraw liquidity",
                anyhow::Error::new(liquidity.clone()),
                Some(liquidity),
            ),
            (
                "deposit below the minimum from the vault preview",
                below_minimum,
                Some(VaultError::BelowMinimumDeposit {
                    amount: 9,
                    minimum: 100,
                }),
            ),
            (
                "vault overflow",
                anyhow::Error::new(VaultError::Overflow { context: "test" }),
                None,
            ),
            ("non-vault error", anyhow::anyhow!("rpc down"), None),
        ] {
            let got = blocking(&error);
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }

    /// `deposit_cap` is the largest deposit minting at most `room` shares:
    /// one token more mints `room + 1`.
    #[test]
    fn deposit_cap_is_the_largest_amount_minting_within_room() {
        let capped = deposit_cap(&STATE, 1_000, 100);
        assert_eq!(capped, 151, "cap for a room of 100 shares");
        for (label, amount, want) in [
            ("at the cap", capped, 100),
            ("one above the cap", capped + 1, 101),
        ] {
            let got = STATE.deposit(amount).map(|outcome| outcome.shares).ok();
            assert_eq!(got, Some(want), "{label}: got {got:?}, want {want}");
        }
        for (label, room, want) in [
            ("unbounded room keeps the amount", u64::MAX, 1_000),
            ("no room", 0, 0),
        ] {
            let got = deposit_cap(&STATE, 1_000, room);
            assert_eq!(got, want, "{label}: got {got}, want {want}");
        }
    }

    /// `withdraw_cap` is the largest withdrawal paying at most `room` tokens:
    /// one share more pays above `room`.
    #[test]
    fn withdraw_cap_is_the_largest_amount_paying_within_room() {
        let capped = withdraw_cap(&STATE, 1_000, 100);
        assert_eq!(capped, 67, "cap for a room of 100 tokens");
        for (label, amount, want) in [
            ("at the cap", capped, 100),
            ("one above the cap", capped + 1, 102),
        ] {
            let got = STATE.withdraw(amount).map(|outcome| outcome.tokens).ok();
            assert_eq!(got, Some(want), "{label}: got {got:?}, want {want}");
        }
        for (label, amount, room, want) in [
            (
                "unbounded room stops at the issued shares",
                5_000,
                u64::MAX,
                2_000,
            ),
            ("no room", 1_000, 0, 0),
        ] {
            let got = withdraw_cap(&STATE, amount, room);
            assert_eq!(got, want, "{label}: got {got}, want {want}");
        }
    }

    fn range(min: u64, max: u64) -> Option<TargetRange> {
        TargetRange::new(min, max).ok()
    }

    /// Missing shares are sized with the pure AUM ratio, not a withdrawal
    /// preview: more missing shares than the vault has issued, which a
    /// withdrawal would refuse with `SharesExceedIssued`, still size a
    /// deposit, and a withdrawal penalty does not under-size it.
    #[test]
    fn missing_shares_size_the_deposit_with_the_aum_ratio() {
        let pair = Pair::new(Address::new_unique(), STATE.token_mint);
        let state = VaultState {
            withdrawal_penalty_bps: 1_000,
            ..STATE
        };
        // Shares middle 5_000, so 5_000 are missing: worth 7_500 tokens.
        let need = rebalance_need(
            &pair,
            Holdings {
                collateral: 100_000,
                shares: 0,
            },
            None,
            range(4_000, 6_000),
            &state,
        );
        let want = RebalanceRequest {
            pair,
            kind: RebalanceKind::Shares,
            amount: 7_500,
        };
        let got = need.ok();
        assert_eq!(
            got,
            Some(RebalanceNeed::Order(want)),
            "got {got:?}, want an order of {want:?}"
        );
    }

    /// The backoff doubles per trigger under one key and stops at the cap.
    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let base = Duration::from_secs(1);
        for (label, triggers, want) in [
            ("first trigger", 1, Duration::from_secs(1)),
            ("third trigger", 3, Duration::from_secs(4)),
            ("tenth trigger reaches the cap", 10, REBALANCE_BACKOFF_CAP),
            (
                "u32::MAX triggers stay at the cap",
                u32::MAX,
                REBALANCE_BACKOFF_CAP,
            ),
        ] {
            let got = backoff_delay(base, triggers);
            assert_eq!(got, want, "{label}: got {got:?}, want {want:?}");
        }
    }
}

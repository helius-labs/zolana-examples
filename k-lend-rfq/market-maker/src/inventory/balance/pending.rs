//! Balances the maker has committed but that are not yet reflected in its
//! indexed UTXOs: amounts queued operations will spend, change of in-flight
//! steps, and the in- and outflows of admitted fills. Each amount is counted
//! from admission until the indexed balance takes it over, and never twice.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};

use solana_address::Address;
use solana_signature::Signature;

use k_lend_rfq_sdk::swap::SwapError;

use super::reservations::Reservations;
use crate::{
    config::TargetRange,
    error::MakerError,
    transactions::steps::{OperationId, StepId},
};

/// The maker's balance bookkeeping shared by the api and the coordinator.
pub struct PendingBalance {
    /// The indexed UTXOs and which steps hold them.
    pub reservations: Arc<Reservations>,
    /// Per asset, the amount queued operations will spend.
    queued: Mutex<HashMap<Address, u64>>,
    /// Per in-flight step, the asset and amount of the maker's own outputs
    /// (change, shielded vault proceeds) it will create.
    incoming: Mutex<HashMap<StepId, (Address, u64)>>,
    fills: Mutex<FillFlows>,
    next_operation: AtomicU64,
    /// Signatures of landed rebalances, in landing order.
    rebalances: Mutex<Vec<Signature>>,
    triggered_rebalances: AtomicUsize,
    range_refusals: AtomicUsize,
}

/// What a user pays the maker in an admitted fill, counted in the maker's net
/// balance until the payment is indexed.
///
/// Lifecycle: recorded by `queue_fill` when the fill is admitted, without
/// the outputs `Reservations` already tracks; each output
/// is removed by `inflow_landed` when sync indexes its UTXO into
/// `Reservations` (from then on `Reservations::balance` counts it); the inflow
/// is removed once its last output lands, or as a whole by `drop_fill` when the
/// fill does not land. `finish_fill` leaves it in place: the user's UTXO is
/// only in the maker's balance once sync has indexed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inflow {
    pub asset: Address,
    pub outputs: Vec<InflowOutput>,
}

/// One output of the user's transfer that pays the maker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InflowOutput {
    pub utxo_hash: [u8; 32],
    pub amount: u64,
}

impl Inflow {
    /// Amount of the outputs that have not landed yet. Saturates at
    /// `u64::MAX` on purpose: it only feeds the net balance view and the
    /// inflow range check, where a saturated amount is refused as above the
    /// range maximum; `check_user_transfer` already rejects payments whose
    /// outputs overflow.
    pub fn amount(&self) -> u64 {
        self.outputs
            .iter()
            .fold(0u64, |total, output| total.saturating_add(output.amount))
    }
}

/// What the maker pays the user in an admitted fill, subtracted from the net
/// balance until whichever comes first: the fill's selected inputs
/// (`fill_inputs`) leave `Reservations` (sync or landing removed them as
/// spent, so `Reservations::balance` already shows the payment), the fill
/// lands (`finish_fill`), or it is dropped (`drop_fill`).
#[derive(Clone, Copy)]
pub struct Outflow {
    pub asset: Address,
    pub amount: u64,
}

/// The target ranges a fill is admitted against: of the asset it receives
/// and of the asset it pays.
pub struct FillRanges {
    pub inflow: Option<TargetRange>,
    pub outflow: Option<TargetRange>,
}

/// The in- and outflows of admitted fills, keyed by operation.
#[derive(Default)]
struct FillFlows {
    inflows: HashMap<OperationId, Inflow>,
    outflows: HashMap<OperationId, Outflow>,
    /// The commitments of the maker inputs the fill's current step spends,
    /// set once inputs are selected (`fill_inputs`).
    outflow_inputs: HashMap<OperationId, Vec<[u8; 32]>>,
}

impl FillFlows {
    /// Reservations balance of `asset`, plus the inflow outputs not yet
    /// indexed by sync, minus the outflows still counting.
    ///
    /// An inflow output is counted from fill admission until sync indexes its
    /// UTXO (`inflow_landed`), after which `Reservations::balance` counts it
    /// instead; it is never counted twice, and spending the landed UTXO (for
    /// example in a rebalance) cannot leave a stale inflow behind.
    ///
    /// An outflow is subtracted from fill admission until its selected inputs
    /// leave `Reservations` or `finish_fill` runs, whichever comes first
    /// (`outflow_counts`). Once the inputs are removed the payment already
    /// shows in `Reservations::balance`, so a sync that runs between landing
    /// and `finish_fill` does not subtract it twice.
    ///
    /// The sums saturate on purpose: this is an infallible view for range
    /// checks, and the amounts of one mint never exceed its `u64` supply.
    fn net_balance(&self, reservations: &Reservations, asset: &Address) -> u64 {
        let incoming = self
            .inflows
            .values()
            .filter(|inflow| inflow.asset == *asset)
            .fold(0u64, |total, inflow| total.saturating_add(inflow.amount()));
        let outgoing = self
            .outflows
            .iter()
            .filter(|(operation, outflow)| {
                outflow.asset == *asset && self.outflow_counts(reservations, operation)
            })
            .fold(0u64, |total, (_, outflow)| {
                total.saturating_add(outflow.amount)
            });
        reservations
            .balance(asset)
            .saturating_add(incoming)
            .saturating_sub(outgoing)
    }

    /// Whether the outflow of `operation` still counts: no inputs selected
    /// yet, or every selected input still tracked by `reservations`.
    fn outflow_counts(&self, reservations: &Reservations, operation: &OperationId) -> bool {
        self.outflow_inputs
            .get(operation)
            .is_none_or(|inputs| inputs.iter().all(|input| reservations.tracks(input)))
    }

    fn remove(&mut self, operation: &OperationId) {
        self.inflows.remove(operation);
        self.outflows.remove(operation);
        self.outflow_inputs.remove(operation);
    }
}

/// Locks `mutex`, recovering the data of a poisoned lock: every update
/// under these locks leaves the data consistent.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The direction an amount moves relative to the maker's balance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// The maker receives it.
    In,
    /// The maker pays it.
    Out,
}

/// Whether moving `amount` of `asset` in or out of `balance` (`flow`) keeps
/// it inside `range`: an inflow may not push it above `range.max()`, an
/// outflow may not pull it below `range.min()`. Either fails with
/// `SwapError::OutsideTargetRange`. No range means no limit.
pub fn range_check(
    asset: Address,
    balance: u64,
    amount: u64,
    flow: Flow,
    range: Option<TargetRange>,
) -> Result<(), SwapError> {
    let Some(range) = range else {
        return Ok(());
    };
    let (balance_after, outside) = match flow {
        Flow::In => {
            let after = balance.saturating_add(amount);
            (after, after > range.max())
        }
        Flow::Out => {
            let after = balance.saturating_sub(amount);
            (after, after < range.min())
        }
    };
    if outside {
        return Err(SwapError::OutsideTargetRange {
            asset,
            balance_after,
            min: range.min(),
            max: range.max(),
        });
    }
    Ok(())
}

impl PendingBalance {
    pub fn new(reservations: Arc<Reservations>) -> Self {
        Self {
            reservations,
            queued: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            fills: Mutex::new(FillFlows::default()),
            next_operation: AtomicU64::new(0),
            rebalances: Mutex::new(Vec::new()),
            triggered_rebalances: AtomicUsize::new(0),
            range_refusals: AtomicUsize::new(0),
        }
    }

    /// Commits `amount` of `asset` to a queued operation; the caller
    /// releases it with `unqueue` when the operation leaves the queue.
    ///
    /// What admission may count, its single definition: every maker UTXO of
    /// `asset` no step holds, indexed or not
    /// (`Reservations::unreserved_balance`), plus the own outputs in-flight
    /// steps will create (`expect`), minus what queued operations already
    /// committed. An unindexed UTXO is the change of a landed step: it is
    /// counted from landing on, so a fill admitted between landing and sync
    /// is backlogged until sync indexes the change instead of refused. Each
    /// amount is in exactly one term: `expect` covers a step's outputs until
    /// it lands (`settle`), `Reservations` from then on.
    ///
    /// Errors with `MakerError::InsufficientBalance` if `amount` exceeds what
    /// admission may count, and `MakerError::AmountOverflow` if a sum
    /// overflows. The subtraction of the queued amount saturates on purpose:
    /// queued operations can exceed the balance after UTXOs left it, which
    /// leaves nothing available.
    pub fn queue(&self, asset: Address, amount: u64) -> Result<(), MakerError> {
        // Reservations before `incoming`: a step that lands is settled before
        // its outputs are tracked, so this order never counts them twice.
        let unreserved = self.reservations.unreserved_balance(&asset)?;
        let incoming = self.incoming(&asset)?;
        let mut queued = locked(&self.queued);
        let entry = queued.entry(asset).or_default();
        let available = unreserved
            .checked_add(incoming)
            .ok_or(MakerError::AmountOverflow {
                context: "admission balance",
            })?
            .saturating_sub(*entry);
        if available < amount {
            return Err(MakerError::InsufficientBalance {
                asset,
                available,
                requested: amount,
            });
        }
        *entry = entry
            .checked_add(amount)
            .ok_or(MakerError::AmountOverflow {
                context: "queued amount",
            })?;
        Ok(())
    }

    /// Releases an amount committed by `queue`.
    pub fn unqueue(&self, asset: Address, amount: u64) {
        let mut queued = locked(&self.queued);
        if let Some(entry) = queued.get_mut(&asset) {
            *entry = entry.saturating_sub(amount);
        }
    }

    /// Admits a fill's flows under `operation`, checked against the net
    /// balance under one lock so concurrent fills cannot both pass: first the
    /// outflow against `ranges.outflow`, then the inflow against
    /// `ranges.inflow`, each failing with `SwapError::OutsideTargetRange`
    /// (wrapped in `MakerError::Swap`).
    ///
    /// Inflow outputs `Reservations` already tracks are left out: they
    /// already landed (for example a user transfer reused for a second
    /// order) and `Reservations::balance` counts them, so recording them would
    /// count them twice until `drop_fill`.
    pub fn queue_fill(
        &self,
        operation: OperationId,
        mut inflow: Inflow,
        outflow: Outflow,
        ranges: FillRanges,
    ) -> Result<(), MakerError> {
        let mut fills = locked(&self.fills);
        inflow
            .outputs
            .retain(|output| !self.reservations.tracks(&output.utxo_hash));
        let out_balance = fills.net_balance(&self.reservations, &outflow.asset);
        range_check(
            outflow.asset,
            out_balance,
            outflow.amount,
            Flow::Out,
            ranges.outflow,
        )?;
        let in_balance = fills.net_balance(&self.reservations, &inflow.asset);
        range_check(
            inflow.asset,
            in_balance,
            inflow.amount(),
            Flow::In,
            ranges.inflow,
        )?;
        fills.inflows.insert(operation, inflow);
        fills.outflows.insert(operation, outflow);
        Ok(())
    }

    /// Records the commitments of the maker inputs the fill `operation`
    /// spends, replacing those of an earlier attempt. From the moment any of
    /// them leaves `Reservations` the outflow stops counting
    /// (`FillFlows::net_balance`). Ignored for an operation without an
    /// outflow.
    pub fn fill_inputs(&self, operation: OperationId, inputs: Vec<[u8; 32]>) {
        let mut fills = locked(&self.fills);
        if fills.outflows.contains_key(&operation) {
            fills.outflow_inputs.insert(operation, inputs);
        }
    }

    /// The fill landed: its outflow is now visible as spent inputs, if sync
    /// has not already removed them. The inflow stays until sync indexes the
    /// user's outputs (`inflow_landed`).
    pub fn finish_fill(&self, operation: OperationId) {
        let mut fills = locked(&self.fills);
        fills.outflows.remove(&operation);
        fills.outflow_inputs.remove(&operation);
    }

    /// The fill will not land: both its flows are removed.
    pub fn drop_fill(&self, operation: OperationId) {
        locked(&self.fills).remove(&operation);
    }

    /// Called by sync for every UTXO it indexes into `Reservations`. Removes
    /// the matching output from whichever admitted inflow holds it, and drops
    /// the inflow once all its outputs have landed. From here on the payment
    /// is counted by `Reservations::balance`, so `remove_spent` needs no
    /// knowledge of inflows. UTXOs that are not a user's payment are ignored.
    pub fn inflow_landed(&self, utxo_hash: &[u8; 32]) {
        let mut fills = locked(&self.fills);
        fills.inflows.retain(|_, inflow| {
            inflow
                .outputs
                .retain(|output| output.utxo_hash != *utxo_hash);
            !inflow.outputs.is_empty()
        });
    }

    /// Records the maker's own outputs `step` will create, counted by
    /// `queue` as available until `settle`.
    pub fn expect(&self, step: StepId, asset: Address, amount: u64) {
        locked(&self.incoming).insert(step, (asset, amount));
    }

    /// Forgets the outputs recorded by `expect` for `step`: at landing, when
    /// they enter `Reservations` as tracked (unindexed) UTXOs that `queue`
    /// counts from then on, or when the step is released. At landing it must
    /// run before the outputs are inserted, so `queue` never counts them in
    /// both places.
    pub fn settle(&self, step: StepId) {
        locked(&self.incoming).remove(&step);
    }

    /// See `FillFlows::net_balance`.
    pub fn net_balance(&self, asset: &Address) -> u64 {
        locked(&self.fills).net_balance(&self.reservations, asset)
    }

    /// A fresh operation id, unique for the process lifetime.
    pub fn next_operation(&self) -> OperationId {
        self.next_operation.fetch_add(1, Ordering::Relaxed)
    }

    /// Records a landed rebalance.
    pub fn record_rebalance(&self, signature: Signature) {
        locked(&self.rebalances).push(signature);
    }

    /// Counts an automatic rebalance when it is queued.
    pub fn record_triggered_rebalance(&self) {
        self.triggered_rebalances.fetch_add(1, Ordering::Relaxed);
    }

    /// Automatic rebalances queued since start, landed or not.
    pub fn triggered_rebalances(&self) -> usize {
        self.triggered_rebalances.load(Ordering::Relaxed)
    }

    /// Counts a range check that found a pair out of range and queued no
    /// rebalance for it (`RangeOutcome::Conflict` or `RangeOutcome::Skip`).
    pub fn record_range_refusal(&self) {
        self.range_refusals.fetch_add(1, Ordering::Relaxed);
    }

    /// Range checks since start that found a pair out of range and queued
    /// no rebalance, whether or not they logged a warning.
    pub fn range_refusals(&self) -> usize {
        self.range_refusals.load(Ordering::Relaxed)
    }

    /// Signatures of the landed rebalances, in landing order.
    pub fn rebalances(&self) -> Vec<Signature> {
        locked(&self.rebalances).clone()
    }

    /// Sum of the outputs of `asset` recorded by `expect`; errors with
    /// `MakerError::AmountOverflow` if it overflows `u64`.
    fn incoming(&self, asset: &Address) -> Result<u64, MakerError> {
        locked(&self.incoming)
            .values()
            .filter(|(incoming, _)| incoming == asset)
            .try_fold(0u64, |total, (_, amount)| total.checked_add(*amount))
            .ok_or(MakerError::AmountOverflow {
                context: "incoming own outputs",
            })
    }
}

#[cfg(test)]
mod tests {
    use zolana_keypair::PublicKey;
    use zolana_transaction::{Data, Mint, Utxo, WalletUtxo};

    use super::*;
    use crate::inventory::balance::reservations::TrackedUtxo;

    const PAID: Address = Address::new_from_array([1; 32]);
    const SENT: Address = Address::new_from_array([2; 32]);
    const FIRST: InflowOutput = InflowOutput {
        utxo_hash: [11; 32],
        amount: 10,
    };
    const SECOND: InflowOutput = InflowOutput {
        utxo_hash: [12; 32],
        amount: 20,
    };

    const NO_RANGES: FillRanges = FillRanges {
        inflow: None,
        outflow: None,
    };

    /// A maker UTXO of `asset` with commitment and nullifier `hash`.
    fn tracked(
        asset: Address,
        hash: [u8; 32],
        amount: u64,
        leaf_index: Option<u64>,
    ) -> TrackedUtxo {
        TrackedUtxo {
            wallet: WalletUtxo {
                utxo: Utxo {
                    owner: PublicKey::zeroed(),
                    asset: Mint::new(asset, 0),
                    amount,
                    blinding: [0; 32],
                    ring_program_id: None,
                    data: Data::default(),
                },
                nullifier_pubkey: [0; 32],
                utxo_hash: hash,
                nullifier: hash,
                data_hash: None,
                ring_data_hash: None,
                tree_id: 0,
                leaf_index: 0,
                slot: 0,
                tx_signature: Signature::default(),
                slot_index: 0,
            },
            leaf_index,
        }
    }

    /// A fill's inflow counts towards the net balance until its UTXO lands,
    /// output by output, and is forgotten once every output has landed.
    #[test]
    fn net_balance_drops_inflow_once_its_utxo_lands() {
        let pending = PendingBalance::new(Arc::new(Reservations::default()));
        let operation = pending.next_operation();
        pending
            .queue_fill(
                operation,
                Inflow {
                    asset: PAID,
                    outputs: vec![FIRST, SECOND],
                },
                Outflow {
                    asset: SENT,
                    amount: 5,
                },
                NO_RANGES,
            )
            .expect("fill without ranges is admitted");
        for (label, landed, want) in [
            ("no output landed", None, 30),
            ("first output landed", Some(FIRST.utxo_hash), 20),
            ("both outputs landed", Some(SECOND.utxo_hash), 0),
        ] {
            if let Some(utxo_hash) = landed {
                pending.inflow_landed(&utxo_hash);
            }
            let got = pending.net_balance(&PAID);
            assert_eq!(got, want, "{label}: got {got}, want {want}");
        }
        let open_inflows = locked(&pending.fills).inflows.len();
        assert_eq!(open_inflows, 0, "open inflows: got {open_inflows}, want 0");
    }

    /// An outflow is subtracted until its inputs leave the reservations or
    /// the fill finishes, whichever comes first: a sync that removes the
    /// spent input and indexes the change before `finish_fill` runs does not
    /// subtract the payment a second time.
    #[test]
    fn outflow_stops_counting_once_its_inputs_leave() {
        const INPUT: [u8; 32] = [21; 32];
        const CHANGE: [u8; 32] = [22; 32];
        let reservations = Arc::new(Reservations::default());
        reservations.insert(tracked(SENT, INPUT, 100, Some(0)));
        let pending = PendingBalance::new(reservations.clone());
        let operation = pending.next_operation();
        pending
            .queue_fill(
                operation,
                Inflow {
                    asset: PAID,
                    outputs: vec![],
                },
                Outflow {
                    asset: SENT,
                    amount: 30,
                },
                NO_RANGES,
            )
            .expect("fill without ranges is admitted");
        pending.fill_inputs(operation, vec![INPUT]);
        let admitted = pending.net_balance(&SENT);
        assert_eq!(admitted, 70, "admitted: got {admitted}, want 70");
        // Sync sees the landed swap before `on_confirmed` runs.
        reservations.remove_spent(&[INPUT]);
        reservations.insert(tracked(SENT, CHANGE, 70, Some(1)));
        let synced = pending.net_balance(&SENT);
        assert_eq!(
            synced, 70,
            "synced before finish_fill: got {synced}, want 70"
        );
        pending.finish_fill(operation);
        let finished = pending.net_balance(&SENT);
        assert_eq!(finished, 70, "finished: got {finished}, want 70");
    }

    /// A landed step's change is counted by admission from landing on: first
    /// as the step's expected outputs, then as an unindexed tracked UTXO, and
    /// never in both places.
    #[test]
    fn queue_counts_change_between_landing_and_sync() {
        const STEP: StepId = 7;
        const CHANGE: [u8; 32] = [31; 32];
        let reservations = Arc::new(Reservations::default());
        let pending = PendingBalance::new(reservations.clone());
        pending.expect(STEP, SENT, 50);
        pending
            .queue(SENT, 50)
            .expect("in-flight change is admitted");
        pending.unqueue(SENT, 50);
        // Landing: the step is settled, then its change is tracked unindexed.
        pending.settle(STEP);
        reservations.insert(tracked(SENT, CHANGE, 50, None));
        pending
            .queue(SENT, 50)
            .expect("unindexed change of a landed step is admitted");
        let refused = pending.queue(SENT, 1);
        assert!(
            matches!(
                refused,
                Err(MakerError::InsufficientBalance {
                    available: 0,
                    requested: 1,
                    ..
                })
            ),
            "got {refused:?}, want InsufficientBalance {{ available: 0, requested: 1 }}"
        );
    }

    /// An inflow output that `Reservations` already tracks (it landed
    /// before) is not recorded again, so it is not counted twice.
    #[test]
    fn queue_fill_skips_outputs_already_tracked() {
        let reservations = Arc::new(Reservations::default());
        reservations.insert(tracked(PAID, FIRST.utxo_hash, FIRST.amount, Some(0)));
        let pending = PendingBalance::new(reservations);
        pending
            .queue_fill(
                pending.next_operation(),
                Inflow {
                    asset: PAID,
                    outputs: vec![FIRST, SECOND],
                },
                Outflow {
                    asset: SENT,
                    amount: 0,
                },
                NO_RANGES,
            )
            .expect("fill without ranges is admitted");
        let got = pending.net_balance(&PAID);
        let want = FIRST.amount + SECOND.amount;
        assert_eq!(got, want, "net balance: got {got}, want {want}");
    }
}

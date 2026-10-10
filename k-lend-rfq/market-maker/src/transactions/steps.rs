//! Steps: the unit the coordinator proves, sends and confirms. One step is one
//! market maker transaction (a fill's transfer, a consolidation, a rebalance).
//! A step holds its input reservations from admission until it leaves the
//! table, confirmed or released.

use std::{collections::HashMap, time::Instant};

use solana_address::Address;
use solana_instruction::Instruction;
use solana_message::VersionedMessage;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use tokio::sync::oneshot;
use zolana_program::instruction::TransactInterfaceTransferAccounts;
use zolana_transaction::{instructions::transact::SppProofInputs, WalletUtxo};

use super::{
    send::Sent,
    shield::{ShieldPlan, REBALANCE_COMPUTE_BUDGET},
};
use crate::{error::MarketMakerError, swap::fill::SWAP_COMPUTE_BUDGET};
use k_lend_rfq_sdk::{pair::VaultState, swap::OrderId};

/// Identifies a step for the coordinator's lifetime.
pub type StepId = u64;
/// Identifies an api operation (fill, consolidation, rebalance); one
/// operation may run several steps across retries.
pub type OperationId = u64;

/// The transaction a step builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// The market maker's transfer in a swap; sent only after the user signs.
    Fill,
    /// A transfer of the market maker to itself.
    Consolidate,
    /// An unshielding transfer followed by a kVault instruction and a shield.
    Rebalance,
}

/// The witness a step's proof is generated from.
#[derive(Clone)]
pub struct ProofWork {
    pub inputs: SppProofInputs,
    pub interface_accounts: Vec<TransactInterfaceTransferAccounts>,
}

/// Where a step is. A send that never reached the rpc returns the step to
/// `Proven`, and a re-prove to `Proving`; otherwise states advance in
/// declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Proving,
    /// Proof and instruction ready; waiting to be sent.
    Proven,
    /// A fill whose swap message is with the user.
    AwaitingSignature,
    Sending,
    /// At least one send reached the rpc; awaiting confirmation.
    Sent,
    Confirmed,
}

impl StepState {
    /// Nothing was sent, so dropping the step cannot leave a transaction in
    /// flight.
    pub fn is_unsent(self) -> bool {
        matches!(self, Self::Proving | Self::Proven | Self::AwaitingSignature)
    }

    /// A transaction may be in flight.
    pub fn is_on_chain_pending(self) -> bool {
        matches!(self, Self::Sending | Self::Sent)
    }
}

/// The fill-specific state of a `StepKind::Fill` step.
pub struct FillTransfer {
    /// The order being filled; its marker instruction ends the swap message,
    /// and a rejected marker reports `SwapError::OrderAlreadyFilled` for it.
    pub order: OrderId,
    /// The rent minimum the marker account is funded with.
    pub marker_lamports: u64,
    /// The user's proven transfer, first in the swap message.
    pub user_transfer: Instruction,
    /// The order's expiry (`OpenOrder::expires_at`), carried as an absolute
    /// instant so that backlog, proving and requeues cannot push it out.
    /// No proof is started and no message offered once it has passed.
    pub deadline: Instant,
    /// Nullifiers of the market maker inputs, reported to the user.
    pub spends: Vec<[u8; 32]>,
    /// The unsigned swap message, set by `offer_fill`.
    pub message: Option<VersionedMessage>,
    /// Last block height at which the message's blockhash is valid.
    pub last_valid_block_height: u64,
    /// The co-signing deadline, set by `offer_fill` to `deadline` when the
    /// message is offered: the co-sign deadline is the order's expiry, never
    /// later.
    pub expires_at: Option<Instant>,
    /// The fully signed swap, set by `on_settle`.
    pub transaction: Option<VersionedTransaction>,
    /// Answers the `settle` call once the swap lands or fails.
    pub settle: Option<oneshot::Sender<Result<Signature, MarketMakerError>>>,
}

/// The tail of a kVault operation: the kVault instruction and the shield of
/// the asset it pays to the market maker's public account.
///
/// The `VaultState` preview the operation was sized with is an estimate: the
/// kVault program charges fees and accrues interest at execution, so the
/// minted or redeemed amount differs from it. The shield amount is therefore
/// not fixed when the operation is scheduled; it is resolved from a
/// simulation right before sending (`TailShield::resolve`).
#[derive(Clone)]
pub struct TailShield {
    /// The kVault deposit or withdraw.
    pub vault_instruction: Instruction,
    /// The mint the kVault instruction pays out.
    pub asset: Address,
    /// The market maker's associated token account of `asset`.
    pub asset_account: Address,
    /// The balance of `asset_account` when the operation was scheduled. Up
    /// to a dust cap of it (residuals of earlier tails) is swept by this
    /// shield; the rest is the operator's public float (`TailShield::amount`).
    pub before: u64,
    /// Splits the shielded amount into UTXOs.
    pub plan: ShieldPlan,
    /// Further UTXOs of other assets the shield instruction carries.
    pub also_shield: Vec<(Address, u64)>,
    /// The compute units the transaction carrying `vault_instruction`
    /// requests, grown with the vault's reserves (`vault_compute_units`).
    pub compute_units: u32,
}

/// One market maker transaction in progress.
pub struct Step {
    pub id: StepId,
    pub kind: StepKind,
    /// The asset whose UTXOs the step spends.
    pub asset: Option<Address>,
    /// The api operation waiting on the step; `None` for upkeep.
    pub operation: Option<OperationId>,
    /// Commitments of the reserved inputs.
    pub inputs: Vec<[u8; 32]>,
    /// The market maker's own outputs the step creates (change, consolidation).
    pub expected_outputs: Vec<WalletUtxo>,
    /// The witness; the prove task proves a copy, so a retry reproves it.
    pub proof: ProofWork,
    /// The proven `transact`, set by the prove task.
    pub instruction: Option<Instruction>,
    /// Instructions sent after `instruction` in the same transaction. A step
    /// with a `tail_shield` holds its kVault instruction here until the prove
    /// task resolves the shield, then the kVault instruction and the shield.
    pub tail: Vec<Instruction>,
    pub tail_shield: Option<TailShield>,
    /// The vault state a rebalance was previewed against.
    pub vault_before: Option<VaultState>,
    pub fill: Option<FillTransfer>,
    /// Every send that reached the rpc, oldest first.
    pub sends: Vec<Sent>,
    /// The last resend was rejected; the step waits for its earlier sends.
    pub resend_failed: bool,
    /// Consecutive sends that did not reach the rpc (`SendOutcome::NotSent`);
    /// non-zero while a backoff retry owns the step's next send.
    pub send_failures: usize,
    pub state: StepState,
    /// Proofs attempted so far; capped by `PROVE_ATTEMPTS`.
    pub prove_attempts: u32,
}

impl Step {
    /// A fresh step in `StepState::Proving`.
    pub fn new(id: StepId, kind: StepKind, proof: ProofWork) -> Self {
        Self {
            id,
            kind,
            asset: None,
            operation: None,
            inputs: Vec::new(),
            expected_outputs: Vec::new(),
            proof,
            instruction: None,
            tail: Vec::new(),
            tail_shield: None,
            vault_before: None,
            fill: None,
            sends: Vec::new(),
            resend_failed: false,
            send_failures: 0,
            state: StepState::Proving,
            prove_attempts: 0,
        }
    }

    /// An upkeep consolidation: no operation waits on it, so a fill may
    /// preempt it.
    pub fn is_upkeep(&self) -> bool {
        self.operation.is_none() && self.kind == StepKind::Consolidate
    }

    /// The compute unit limit the step's transaction requests.
    pub fn compute_units(&self) -> u32 {
        match self.kind {
            StepKind::Fill => SWAP_COMPUTE_BUDGET.cu_limit,
            StepKind::Consolidate | StepKind::Rebalance => self
                .tail_shield
                .as_ref()
                .map_or(REBALANCE_COMPUTE_BUDGET.cu_limit, |tail| tail.compute_units),
        }
    }
}

/// The coordinator's step table.
#[derive(Default)]
pub struct Steps {
    steps: HashMap<StepId, Step>,
    next_id: StepId,
}

impl Steps {
    /// A fresh step id; ids start at 1 and wrap only after 2^64 steps.
    pub fn next_id(&mut self) -> StepId {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    pub fn insert(&mut self, step: Step) {
        self.steps.insert(step.id, step);
    }

    pub fn get(&self, id: StepId) -> Option<&Step> {
        self.steps.get(&id)
    }

    pub fn get_mut(&mut self, id: StepId) -> Option<&mut Step> {
        self.steps.get_mut(&id)
    }

    pub fn remove(&mut self, id: StepId) -> Option<Step> {
        self.steps.remove(&id)
    }

    /// The fill step whose swap message equals `message`.
    pub fn find_fill(&self, message: &VersionedMessage) -> Option<StepId> {
        self.steps
            .values()
            .find(|step| {
                step.fill
                    .as_ref()
                    .is_some_and(|fill| fill.message.as_ref() == Some(message))
            })
            .map(|step| step.id)
    }

    /// Proven non-fill steps without a pending backoff retry: a step with
    /// `send_failures > 0` is sent again by its retry task.
    pub fn ready_to_send(&self) -> Vec<StepId> {
        self.steps
            .values()
            .filter(|step| {
                step.state == StepState::Proven
                    && step.kind != StepKind::Fill
                    && step.send_failures == 0
            })
            .map(|step| step.id)
            .collect()
    }

    /// Steps not yet confirmed.
    pub fn in_flight(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .values()
            .filter(|step| step.state != StepState::Confirmed)
    }

    /// Steps in `StepState::Sent`, the ones the status poll checks.
    pub fn sent(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .values()
            .filter(|step| step.state == StepState::Sent)
    }

    /// Number of fills proven and waiting for the user's signature.
    pub fn awaiting_signature(&self) -> usize {
        self.steps
            .values()
            .filter(|step| step.state == StepState::AwaitingSignature)
            .count()
    }

    /// No step is in flight.
    pub fn is_idle(&self) -> bool {
        self.in_flight().next().is_none()
    }

    /// Some step may have a transaction in flight.
    pub fn has_pending_sends(&self) -> bool {
        self.in_flight()
            .any(|step| step.state.is_on_chain_pending())
    }

    /// Drops confirmed steps from the table.
    pub fn prune_confirmed(&mut self) {
        self.steps
            .retain(|_, step| step.state != StepState::Confirmed);
    }
}

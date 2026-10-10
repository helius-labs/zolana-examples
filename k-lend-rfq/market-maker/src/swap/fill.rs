//! Filling an order: checks the user's proven transfer against the
//! market maker's own order record, schedules the market maker's paying
//! transfer, and compiles the three-instruction swap message the user co-signs.
//! The amounts paid and received always come from the order, never from the
//! request.

use std::{sync::PoisonError, time::Instant};

use anyhow::Result;
use solana_address::Address;
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_message::VersionedMessage;
use zolana_client::{compile_message, ComputeBudgetConfig};
use zolana_keypair::ShieldedAddress;
use zolana_transaction::WalletUtxo;

use k_lend_rfq_sdk::{
    message::transact_data,
    pair::Pair,
    swap::{order_marker_instruction, Fill, OrderId, SwapError, SwapRequest},
    transfer::{Receiver, USER_OUTPUTS},
};

use crate::{
    api::Inner,
    error::MarketMakerError,
    inventory::balance::{
        pending::{Inflow, InflowOutput},
        select::select,
    },
    transactions::budget::MAX_COMPUTE_UNITS,
    transactions::{
        confirm::Retry,
        coordinator::{Coordinator, Operation, OperationOutcome, ScheduleOutcome},
        steps::{FillTransfer, OperationId, StepId, StepKind, StepState},
        transfer::{other_amounts, own_value, plan_transfer, TransferStep},
    },
};

/// The Solana per-transaction compute unit maximum: a swap holds two proof
/// verifications. A V1 message carries the budget in its header config, so
/// it adds no instruction to the swap message.
pub const SWAP_COMPUTE_BUDGET: ComputeBudgetConfig = ComputeBudgetConfig::new(MAX_COMPUTE_UNITS);
/// Index of the order marker instruction in the swap message, after the
/// user's transfer and the market maker's transfer.
pub const MARKER_INSTRUCTION_INDEX: u8 = 2;
/// The custom code of `SystemError::AccountAlreadyInUse` (the first variant
/// of solana-system-interface `SystemError`), which the marker instruction
/// fails with when the order's marker account already exists.
pub const ACCOUNT_ALREADY_IN_USE: u32 = 0;

/// What `fill` returns: the unsigned swap message for the user, plus the
/// market maker's own view of the transfer it contains.
pub struct MarketMakerFill {
    /// The swap message and the instant after which the market maker no longer
    /// co-signs it.
    pub fill: Fill,
    /// Nullifiers of the market maker UTXOs the swap spends.
    pub spent: Vec<[u8; 32]>,
    /// The market maker's change outputs, which appear once the swap lands.
    pub change: Vec<WalletUtxo>,
}

/// A checked fill handed to the coordinator: the order's amounts, the user's
/// transfer and the market maker's inflow from it.
pub struct FillOrder {
    /// The order being filled; its marker ends the swap message.
    pub order: OrderId,
    /// Lamports the marker account is funded with.
    pub marker_lamports: u64,
    /// The asset the market maker pays.
    pub asset: Address,
    /// The order's `amount_out`.
    pub amount: u64,
    /// The user's shielded address, the recipient of `amount`.
    pub recipient: ShieldedAddress,
    /// The user's proven transfer, placed first in the swap message.
    pub user_transfer: Instruction,
    /// The user's outputs to the market maker, tracked as pending until they
    /// land.
    pub inflow: Inflow,
    /// The order's expiry (`OpenOrder::expires_at`). Scheduling and offering
    /// reject the fill with `SwapError::OrderExpired` once it has passed, and
    /// the swap message's co-sign deadline is this instant: the co-sign
    /// deadline is the order's expiry, never later.
    pub deadline: Instant,
}

impl Inner {
    /// Fills the open order `request.order` on `pair` and returns the swap
    /// message for the user to co-sign.
    ///
    /// Checks, in order:
    /// 1. the market maker serves `pair`, else
    ///    `MarketMakerError::PairNotServed`;
    /// 2. the order is open, unexpired and quoted for `pair`
    ///    (`OpenOrders::take`: `SwapError::UnknownOrder`,
    ///    `SwapError::OrderExpired`, `SwapError::OrderPairMismatch`); the
    ///    order is consumed here whatever the outcome of the later checks;
    /// 3. the user's transfer pays the order's `amount_in`
    ///    (`Inner::check_user_transfer`);
    /// 4. admission (`Inner::operation`): both assets stay in range against
    ///    the net balance (`SwapError::OutsideTargetRange`), and the
    ///    unreserved balance covers `amount_out`
    ///    (`MarketMakerError::InsufficientBalance`, returned as
    ///    `SwapError::InsufficientInventory`);
    /// 5. scheduling and proving the market maker's transfer; any failure there
    ///    is returned as its `MarketMakerError` (for example
    ///    `MarketMakerError::FragmentedInventory`), and an order that expires
    ///    before its message is offered as `SwapError::OrderExpired`.
    ///
    /// The co-sign deadline is the order's expiry, never later: the absolute
    /// `OpenOrder::expires_at` travels with the fill, so neither a backlog
    /// nor proving nor a requeue can extend the quote, and the `expires_at`
    /// returned in `Fill` equals it.
    pub async fn fill(&self, pair: &Pair, request: &SwapRequest) -> Result<MarketMakerFill> {
        self.settings().serves(pair)?;
        let order = self.orders.take(request.order, pair)?;
        let quote = order.quote;
        let (asset_in, asset_out) = quote.direction.assets(pair);
        let inflow = self
            .check_user_transfer(asset_in, quote.amount_in, order.max_user_inputs, request)
            .await?;
        let outcome = self
            .operation(Operation::Fill(FillOrder {
                order: request.order,
                marker_lamports: self.marker_lamports,
                asset: asset_out,
                amount: quote.amount_out,
                recipient: request.user,
                user_transfer: request.transfer.clone(),
                inflow,
                deadline: order.expires_at,
            }))
            .await
            .map_err(|error| match error {
                MarketMakerError::InsufficientBalance {
                    asset,
                    available,
                    requested,
                } => anyhow::Error::from(SwapError::InsufficientInventory {
                    asset,
                    required: requested,
                    available,
                }),
                MarketMakerError::Swap(error) => anyhow::Error::from(error),
                other => anyhow::Error::from(other),
            })?;
        match outcome {
            OperationOutcome::Filled(fill) => Ok(fill),
            _ => Err(MarketMakerError::UnexpectedOutcome { expected: "fill" }.into()),
        }
    }

    /// Checks the user's transfer `request.transfer` against the order and
    /// returns the market maker's inflow from it.
    ///
    /// Checks, in order:
    /// 1. it is a zolana `transact`, else `SwapError::UnexpectedTransaction`;
    /// 2. it carries no interface (public) transfer, else
    ///    `SwapError::PublicTransfer`;
    /// 3. it spends at most `max` inputs, the width the offer advertised,
    ///    else `SwapError::UserTransferTooWide`;
    /// 4. it has exactly `USER_OUTPUTS` outputs, else
    ///    `SwapError::UserTransferOutputs`;
    /// 5. every output the market maker can decrypt opens to its commitment,
    ///    else `SwapError::CommitmentMismatch`;
    /// 6. the decrypted outputs of `asset_in` sum to exactly `amount_in`,
    ///    else `SwapError::AmountOverflow` or `SwapError::Underpaid`.
    ///
    /// Width and output count are checked because the market maker's transfer
    /// is sized for the room the user transfer leaves in the transaction.
    ///
    /// Zero-amount outputs are left out of the inflow: sync never tracks a
    /// zero-amount UTXO, so such an output would never land and its inflow
    /// entry would never be dropped.
    async fn check_user_transfer(
        &self,
        asset_in: Address,
        amount_in: u64,
        max: usize,
        request: &SwapRequest,
    ) -> Result<Inflow> {
        let user_transact = transact_data(&request.transfer)?;
        if !user_transact.interface_transfers.is_empty() {
            return Err(SwapError::PublicTransfer {
                count: user_transact.interface_transfers.len(),
            }
            .into());
        }
        if user_transact.inputs.len() > max {
            return Err(SwapError::UserTransferTooWide {
                inputs: user_transact.inputs.len(),
                max,
            }
            .into());
        }
        if user_transact.outputs.len() != USER_OUTPUTS {
            return Err(SwapError::UserTransferOutputs {
                outputs: user_transact.outputs.len(),
                expected: USER_OUTPUTS,
            }
            .into());
        }
        let registry = self
            .registry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let outputs: Vec<_> = Receiver {
            keys: self.identity.keys.as_ref(),
            registry: &registry,
            tree_id: self.identity.tree_id,
        }
        .received_outputs(&user_transact)?
        .into_iter()
        .filter(|(utxo, _)| utxo.asset.asset == asset_in && utxo.amount > 0)
        .collect();
        let received = outputs
            .iter()
            .try_fold(0u64, |total, (utxo, _)| total.checked_add(utxo.amount))
            .ok_or(SwapError::AmountOverflow {
                context: "user outputs to the market maker",
            })?;
        if received != amount_in {
            return Err(SwapError::Underpaid {
                expected: amount_in,
                received,
            }
            .into());
        }
        Ok(Inflow {
            asset: asset_in,
            outputs: outputs
                .into_iter()
                .map(|(utxo, utxo_hash)| InflowOutput {
                    utxo_hash,
                    amount: utxo.amount,
                })
                .collect(),
        })
    }
}

impl Coordinator {
    /// Selects market maker inputs for `order` and schedules the paying
    /// transfer.
    ///
    /// A fill whose order expired (`Instant::now() >= order.deadline`, for
    /// example after waiting in the backlog) is rejected with
    /// `SwapError::OrderExpired` before any input is selected, so no proof is
    /// spent on a dead order.
    ///
    /// The input and output limits come from the transaction budget left next
    /// to the user's transfer. Change is split per the asset's inventory
    /// profile. When no selection fits, `unschedulable` decides between
    /// retrying, backlogging and `MarketMakerError::FragmentedInventory`; a
    /// budget or planning error rejects the operation.
    pub async fn schedule_fill(
        &mut self,
        id: OperationId,
        order: &FillOrder,
    ) -> Result<ScheduleOutcome, MarketMakerError> {
        if Instant::now() >= order.deadline {
            return Err(MarketMakerError::Swap(SwapError::OrderExpired {
                order: order.order,
            }));
        }
        let available = self.services.pending.reservations.available(&order.asset);
        let max_inputs = self
            .services
            .budget
            .max_market_maker_inputs(&order.user_transfer)?;
        let Some(selection) = select(&available, order.amount, max_inputs) else {
            return self
                .unschedulable(order.asset, order.amount, max_inputs)
                .await;
        };
        let max_outputs = self
            .services
            .budget
            .max_market_maker_outputs(&order.user_transfer, selection.inputs.len())?;
        let change = own_value(&selection, order.amount)?;
        let inputs = selection.hashes();
        // Every tracked UTXO of the asset outside the selection counts,
        // reserved ones included.
        let tracked = self.services.pending.reservations.utxos(&order.asset);
        let others = other_amounts(
            tracked.iter().map(|utxo| (&utxo.utxo_hash, utxo.amount)),
            &inputs.iter().copied().collect(),
        );
        let change_parts =
            self.config
                .profile(&order.asset)
                .parts(change, &others, max_outputs.saturating_sub(1));
        let spends = selection
            .inputs
            .iter()
            .map(|input| input.utxo.wallet.nullifier)
            .collect();
        let plan = plan_transfer(selection, order.recipient, order.amount, change_parts)?;
        // The outflow stops counting in the net balance once these inputs
        // leave the reservations, whether sync or landing removes them first.
        self.services.pending.fill_inputs(id, inputs);
        let fill = FillTransfer {
            order: order.order,
            marker_lamports: order.marker_lamports,
            user_transfer: order.user_transfer.clone(),
            deadline: order.deadline,
            spends,
            message: None,
            last_valid_block_height: 0,
            expires_at: None,
            transaction: None,
            settle: None,
        };
        self.schedule_transfer(TransferStep {
            kind: StepKind::Fill,
            asset: order.asset,
            operation: Some(id),
            plan,
            withdrawal: None,
            tail: None,
            vault_before: None,
            fill: Some(fill),
        })
        .await
        .map(|_| ScheduleOutcome::Scheduled)
    }

    /// The outcome for a fill no input selection can pay: `Retry` if unsent
    /// upkeep steps on `asset` were discarded to free their UTXOs,
    /// `Backlogged` if UTXOs of `asset` are reserved by an in-flight step or
    /// not yet indexed, else `MarketMakerError::FragmentedInventory`.
    async fn unschedulable(
        &mut self,
        asset: Address,
        amount: u64,
        max_inputs: usize,
    ) -> Result<ScheduleOutcome, MarketMakerError> {
        if self.preempt_upkeep(&asset).await {
            return Ok(ScheduleOutcome::Retry);
        }
        if self.waits_for_utxos(&asset) {
            return Ok(ScheduleOutcome::Backlogged);
        }
        Err(MarketMakerError::FragmentedInventory {
            asset,
            available: self.services.pending.reservations.balance(&asset),
            requested: amount,
            max_inputs,
        })
    }

    /// Compiles the swap message for fill step `id` and hands it to the waiting
    /// `fill` call. The message holds exactly three instructions: the user's
    /// transfer, the market maker's transfer and, at
    /// `MARKER_INSTRUCTION_INDEX`,
    /// `order_marker_instruction(payer, order, marker_lamports)`. The
    /// market maker signs as fee payer, which is also the marker's seed base,
    /// so the marker adds no signer. `latest` is the blockhash and its last
    /// valid block height fetched by the prove task, so no rpc call runs on the
    /// coordinator loop.
    ///
    /// The message's co-sign deadline (`Fill::expires_at`) is the order's
    /// expiry `FillTransfer::deadline`, never later.
    ///
    /// Failures abort the step: an order that expired while the transfer was
    /// proven (`Instant::now() >= deadline`, `SwapError::OrderExpired`), an
    /// invalid marker (`SwapError::InvalidOrderMarker`), a message over the
    /// transaction budget or a compile error with `Retry::Fail`; a failed
    /// blockhash fetch with `Retry::Requeue`. A step that is gone or no
    /// longer a fill is ignored.
    pub async fn offer_fill(&mut self, id: StepId, latest: Result<(Hash, u64), MarketMakerError>) {
        let parts = self.steps.get(id).and_then(|step| {
            let fill = step.fill.as_ref()?;
            Some((
                fill.user_transfer.clone(),
                step.instruction.clone()?,
                fill.order,
                fill.marker_lamports,
                fill.deadline,
            ))
        });
        let Some((user_transfer, market_maker_transfer, order, marker_lamports, expires_at)) =
            parts
        else {
            return;
        };
        // 1-2. Check the expiry, build the three instructions, check them
        //      against the transaction budget and compile the message.
        let compiled = self.compile_swap(
            order,
            marker_lamports,
            expires_at,
            [user_transfer, market_maker_transfer],
            latest,
        );
        let (message, last_valid_block_height) = match compiled {
            Ok(compiled) => compiled,
            Err((error, retry)) => {
                self.abort(id, error, retry).await;
                return;
            }
        };
        // 3. Record the message and park the step until the user signs or the
        //    order expires.
        let Some(step) = self.steps.get_mut(id) else {
            return;
        };
        let Some(fill) = step.fill.as_mut() else {
            return;
        };
        fill.message = Some(message.clone());
        fill.last_valid_block_height = last_valid_block_height;
        fill.expires_at = Some(expires_at);
        step.state = StepState::AwaitingSignature;
        let transfer = MarketMakerFill {
            fill: Fill {
                message,
                expires_at,
            },
            spent: fill.spends.clone(),
            change: step.expected_outputs.clone(),
        };
        let operation = step
            .operation
            .and_then(|operation| self.scheduled.remove(&operation));
        // 4. Hand the message to the waiting `fill` call. If that call is gone
        //    (the client disconnected), nobody can sign: release the step.
        self.spawn_expiry(id, expires_at);
        let delivered = operation.is_some_and(|operation| {
            operation
                .reply
                .send(Ok(OperationOutcome::Filled(transfer)))
                .is_ok()
        });
        if !delivered {
            self.discard(
                id,
                MarketMakerError::ReservationExpired { step: id },
                Retry::Fail,
            )
            .await;
            self.try_schedule().await;
        }
    }

    /// Steps 1-2 of `offer_fill`, in order: the order has not expired
    /// (`SwapError::OrderExpired`), the marker is valid, and the user's
    /// transfer, the market maker's transfer and the marker fit the transaction
    /// budget, all checked before any signature is asked for and failing
    /// with `Retry::Fail`; the prove task fetched a blockhash (its error with
    /// `Retry::Requeue`); the message compiles against it (`Retry::Fail`).
    /// Returns the message and the blockhash's last valid block height.
    fn compile_swap(
        &self,
        order: OrderId,
        marker_lamports: u64,
        expires_at: Instant,
        [user_transfer, market_maker_transfer]: [Instruction; 2],
        latest: Result<(Hash, u64), MarketMakerError>,
    ) -> Result<(VersionedMessage, u64), (MarketMakerError, Retry)> {
        if Instant::now() >= expires_at {
            return Err((
                MarketMakerError::Swap(SwapError::OrderExpired { order }),
                Retry::Fail,
            ));
        }
        let marker = order_marker_instruction(&self.identity.payer, order, marker_lamports)
            .map_err(|error| (error.into(), Retry::Fail))?;
        let transfers = [user_transfer, market_maker_transfer, marker];
        self.services
            .budget
            .check(&transfers)
            .map_err(|error| (error.into(), Retry::Fail))?;
        let (blockhash, last_valid_block_height) =
            latest.map_err(|error| (error, Retry::Requeue))?;
        let message = compile_message(
            &self.identity.payer,
            &transfers,
            blockhash,
            SWAP_COMPUTE_BUDGET,
        )
        .map_err(|error| (error.into(), Retry::Fail))?;
        Ok((message, last_valid_block_height))
    }
}

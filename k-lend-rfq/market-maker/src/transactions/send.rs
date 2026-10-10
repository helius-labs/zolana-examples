//! Sending: compiling, signing and submitting step transactions, and
//! classifying what the rpc answered. A send whose outcome is unknown is
//! treated as possibly forwarded and polled, never resent blindly on a new
//! blockhash while the old one is valid, so one step lands at most once.

use std::{sync::Arc, time::Instant};

use solana_address::Address;
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_instruction::Instruction;
use solana_instruction_error::InstructionError;
use solana_message::VersionedMessage;
use solana_rpc_client_api::{
    client_error::ErrorKind,
    config::{
        RpcSendTransactionConfig, RpcSimulateTransactionAccountsConfig,
        RpcSimulateTransactionConfig, UiAccountEncoding,
    },
    custom_error::{
        JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE, JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY,
    },
    request::RpcError,
};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;
use solana_transaction_status_client_types::TransactionStatus;
use zolana_client::{
    compile_message, sign_transaction, transaction_size, AsyncRpc, AsyncSolanaRpc, ClientError,
    ComputeBudgetConfig,
};
use zolana_interface::error::ShieldedPoolError;

use k_lend_rfq_sdk::kvault::token_account_amount;

use super::{
    confirm::{Retry, SEND_ATTEMPTS},
    coordinator::{Coordinator, Event},
    steps::{Step, StepId, StepKind, StepState},
};
use crate::{
    error::MarketMakerError, swap::fill::MARKET_MAKER_TRANSFER_INDEX,
    transactions::budget::BudgetError,
};

/// The most signatures one `getSignatureStatuses` request accepts
/// (`MAX_GET_SIGNATURE_STATUSES_QUERY_ITEMS` in agave `rpc-client-types`
/// `request.rs`).
const STATUS_BATCH: usize = 256;
/// Trailing log lines a `SimulationFailed` error carries.
const SIMULATION_LOG_LINES: usize = 8;

/// An unsigned transaction to send with the market maker as fee payer.
#[derive(Clone)]
pub struct SendRequest {
    pub instructions: Vec<Instruction>,
    pub compute_units: u32,
}

impl SendRequest {
    /// The request's compute budget.
    fn budget(&self) -> ComputeBudgetConfig {
        ComputeBudgetConfig::new(self.compute_units)
    }
}

/// A transaction handed to the rpc: its signature and the block height
/// after which its blockhash, and so the transaction, can no longer land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sent {
    pub signature: Signature,
    pub last_valid_block_height: u64,
}

/// What one send attempt achieved.
#[derive(Debug)]
pub enum SendOutcome {
    /// The rpc accepted the transaction.
    Sent(Sent),
    /// No answer arrived; the transaction may have been forwarded, so it is
    /// polled like a sent one.
    OutcomeUnknown { sent: Sent, error: ClientError },
    /// Refused for good; see `Rejection`.
    Rejected(Rejection),
    /// Nothing was forwarded and trying again may succeed.
    NotSent(MarketMakerError),
}

/// Why a transaction was refused before it reached the chain. A rejection is
/// permanent for the transaction as built; sending it again fails the same way.
#[derive(Debug)]
pub enum Rejection {
    /// Instruction `index` failed preflight with custom error `code`.
    Program { index: u8, code: u32 },
    /// Preflight failed with a transaction error that is not a custom
    /// instruction error.
    Transaction(TransactionError),
    /// The rpc answered with JSON-RPC error `code` and no transaction error,
    /// for a reason that does not clear on a retry (see
    /// `classify_send_error`).
    Rpc { code: i64, message: String },
    /// The transaction could not be built or failed a local check, so it was
    /// never handed to the rpc.
    Invalid(MarketMakerError),
}

impl Rejection {
    /// The failing instruction and its custom program error, the input of
    /// the re-prove and already-filled decisions (`handle_failure`).
    pub fn custom_error(&self) -> Option<CustomError> {
        match self {
            Self::Program { index, code } => Some(CustomError {
                index: *index,
                code: *code,
            }),
            Self::Transaction(_) | Self::Rpc { .. } | Self::Invalid(_) => None,
        }
    }
}

impl From<Rejection> for MarketMakerError {
    fn from(rejection: Rejection) -> Self {
        match rejection {
            Rejection::Program { index, code } => MarketMakerError::TransactionRejected {
                error: TransactionError::InstructionError(index, InstructionError::Custom(code)),
            },
            Rejection::Transaction(error) => MarketMakerError::TransactionRejected { error },
            Rejection::Rpc { code, message } => MarketMakerError::RpcRejected { code, message },
            Rejection::Invalid(error) => error,
        }
    }
}

/// Instruction `index` of a transaction failed with custom program error
/// `code`, at preflight or on chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomError {
    pub index: u8,
    pub code: u32,
}

impl CustomError {
    /// The custom instruction error `error` carries, if any.
    pub fn of(error: &TransactionError) -> Option<Self> {
        match error {
            TransactionError::InstructionError(index, InstructionError::Custom(code)) => {
                Some(Self {
                    index: *index,
                    code: *code,
                })
            }
            _ => None,
        }
    }

    /// For a fill step, the market maker's transfer
    /// (`MARKET_MAKER_TRANSFER_INDEX`) failed with
    /// `ShieldedPoolError::NullifierAlreadyQueued` (7043): SPP found the
    /// nullifier PDA of one of its nullifiers already created. The transfer's
    /// own inputs are reserved for this fill, so the nullifier is the order
    /// address, and another transaction already filled the order. The single
    /// definition shared by the preflight and the landed failure path.
    pub fn is_order_already_filled(self, kind: StepKind) -> bool {
        kind == StepKind::Fill
            && self.index == MARKET_MAKER_TRANSFER_INDEX
            && self.code == ShieldedPoolError::NullifierAlreadyQueued as u32
    }
}

/// How a failed `send_transaction` call is handled.
#[derive(Debug)]
enum SendFailure {
    /// The rpc refused the transaction; it was not forwarded.
    Rejected(Rejection),
    /// The client refused the transaction before sending the request.
    Local,
    /// The rpc answered without forwarding the transaction for a reason that
    /// may clear (stale blockhash, unhealthy node); sending again may succeed.
    Transient,
    /// No answer arrived, so the transaction may have been forwarded.
    Unknown,
}

/// What `Coordinator::spawn_send` sends.
enum SendJob {
    /// A settled fill's co-signed transaction, unchanged, with its last
    /// valid block height.
    Resubmit(VersionedTransaction, u64),
    /// Any other step's request, compiled on a fresh blockhash.
    Compile(SendRequest),
}

/// A sent step's state on chain, from `classify`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepStatus {
    /// One of its sends landed and succeeded at `confirmed` commitment.
    Confirmed { signature: Signature },
    /// One of its sends landed and failed at `confirmed` commitment;
    /// `custom` is the failing instruction and its custom program error, if
    /// any.
    Failed {
        reason: String,
        custom: Option<CustomError>,
    },
    /// None landed and the block height is past every send's last valid
    /// block height, so none can land any more.
    Expired,
    /// None landed at `confirmed` yet and one may still.
    Pending,
}

/// Signs with the market maker's signer and talks to the rpc for every send.
pub struct SendQueue {
    /// The concrete rpc: `simulate_token_balance` needs the underlying
    /// client, which `AsyncRpc` does not expose.
    rpc: Arc<AsyncSolanaRpc>,
    signer: Arc<dyn Signer + Send + Sync>,
    payer: Address,
}

impl SendQueue {
    /// A queue sending with `signer` as fee payer.
    pub fn new(rpc: Arc<AsyncSolanaRpc>, signer: Arc<dyn Signer + Send + Sync>) -> Self {
        let payer = signer.pubkey();
        Self { rpc, signer, payer }
    }

    /// Signs `message` with the market maker's signer as fee payer.
    fn sign(&self, message: VersionedMessage) -> Result<VersionedTransaction, ClientError> {
        sign_transaction(message, &[self.signer.as_ref() as &dyn Signer])
    }

    /// Simulates `request` without signature verification, on the rpc's
    /// latest blockhash, and returns the SPL token amount `account` holds
    /// after it (a missing account holds 0). A transaction error in the
    /// simulation, or a failed rpc call, is `SimulationFailed`.
    pub async fn simulate_token_balance(
        &self,
        request: &SendRequest,
        account: Address,
    ) -> Result<u64, MarketMakerError> {
        let message = compile_message(
            &self.payer,
            &request.instructions,
            solana_hash::Hash::default(),
            request.budget(),
        )?;
        let transaction = self.sign(message)?;
        let config = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: Some(CommitmentConfig::confirmed()),
            accounts: Some(RpcSimulateTransactionAccountsConfig {
                encoding: Some(UiAccountEncoding::Base64),
                addresses: vec![account.to_string()],
            }),
            ..RpcSimulateTransactionConfig::default()
        };
        let simulated = self
            .rpc
            .client()
            .simulate_transaction_with_config(&transaction, config)
            .await
            .map_err(|error| MarketMakerError::SimulationFailed {
                error: error.to_string(),
            })?
            .value;
        tracing::debug!(
            units_consumed = ?simulated.units_consumed,
            compute_units = request.compute_units,
            "simulated transaction"
        );
        if let Some(error) = simulated.err {
            let logs = simulated.logs.unwrap_or_default();
            let last_logs = logs.get(logs.len().saturating_sub(SIMULATION_LOG_LINES)..);
            return Err(MarketMakerError::SimulationFailed {
                error: format!("{error:?}; logs: {last_logs:?}"),
            });
        }
        let Some(state) = simulated
            .accounts
            .and_then(|accounts| accounts.into_iter().next())
            .flatten()
        else {
            return Ok(0);
        };
        let account_data = state
            .data
            .decode()
            .ok_or(MarketMakerError::SimulationFailed {
                error: format!("account {account} data does not decode"),
            })?;
        token_account_amount(&account_data).ok_or(MarketMakerError::TokenAccount { account })
    }

    /// Errors with `MarketMakerError::Budget(BudgetError::TransactionTooLarge)`
    /// when `request` does not fit a v1 transaction.
    pub fn check_size(&self, request: &SendRequest) -> Result<(), MarketMakerError> {
        let size = transaction_size(&self.payer, &request.instructions, request.budget())?;
        if size.fits() {
            Ok(())
        } else {
            Err(BudgetError::TransactionTooLarge {
                bytes: size.bytes,
                addresses: size.addresses,
            }
            .into())
        }
    }

    /// The rpc's latest blockhash and its last valid block height.
    pub async fn latest_blockhash(&self) -> Result<(solana_hash::Hash, u64), MarketMakerError> {
        self.rpc
            .get_latest_blockhash()
            .await
            .map_err(MarketMakerError::Rpc)
    }

    /// Compiles `request` on a fresh blockhash, signs it as fee payer and
    /// submits it. A failed blockhash fetch or signature is `NotSent`; a
    /// message that does not compile is `Rejected(Rejection::Invalid)`.
    pub async fn send(&self, request: &SendRequest) -> SendOutcome {
        let (blockhash, last_valid_block_height) = match self.latest_blockhash().await {
            Ok(latest) => latest,
            Err(error) => return SendOutcome::NotSent(error),
        };
        let message = match compile_message(
            &self.payer,
            &request.instructions,
            blockhash,
            request.budget(),
        ) {
            Ok(message) => message,
            Err(error) => return SendOutcome::Rejected(Rejection::Invalid(error.into())),
        };
        let transaction = match self.sign(message) {
            Ok(transaction) => transaction,
            Err(error) => return SendOutcome::NotSent(error.into()),
        };
        self.submit(&transaction, last_valid_block_height).await
    }

    /// Submits a signed transaction with preflight at `confirmed` and no rpc
    /// retries (the coordinator resends itself), and classifies the answer
    /// with `classify_send_error`.
    pub async fn submit(
        &self,
        transaction: &VersionedTransaction,
        last_valid_block_height: u64,
    ) -> SendOutcome {
        let Some(signature) = transaction.signatures.first().copied() else {
            return SendOutcome::Rejected(Rejection::Invalid(MarketMakerError::MissingSignature));
        };
        let sent = Sent {
            signature,
            last_valid_block_height,
        };
        let config = RpcSendTransactionConfig {
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            max_retries: Some(0),
            ..RpcSendTransactionConfig::default()
        };
        match self
            .rpc
            .send_transaction_with_config(transaction, config)
            .await
        {
            Ok(_) => SendOutcome::Sent(sent),
            Err(error) => match classify_send_error(&error) {
                SendFailure::Rejected(rejection) => SendOutcome::Rejected(rejection),
                SendFailure::Local => SendOutcome::Rejected(Rejection::Invalid(error.into())),
                SendFailure::Transient => SendOutcome::NotSent(MarketMakerError::Rpc(error)),
                SendFailure::Unknown => SendOutcome::OutcomeUnknown { sent, error },
            },
        }
    }

    /// The statuses of `signatures`, in order, in batches of `STATUS_BATCH`.
    pub async fn statuses(
        &self,
        signatures: &[Signature],
    ) -> Result<Vec<Option<TransactionStatus>>, MarketMakerError> {
        let mut statuses = Vec::with_capacity(signatures.len());
        for batch in signatures.chunks(STATUS_BATCH) {
            statuses.extend(
                self.rpc
                    .get_signature_statuses(batch.to_vec())
                    .await
                    .map_err(MarketMakerError::Rpc)?,
            );
        }
        Ok(statuses)
    }

    /// The rpc's current block height.
    pub async fn block_height(&self) -> Result<u64, MarketMakerError> {
        self.rpc
            .get_block_height()
            .await
            .map_err(MarketMakerError::Rpc)
    }
}

/// Classifies a failed `send_transaction_with_config` call (preflight on):
///
/// - preflight `InstructionError(index, Custom(code))` -> rejected,
///   `Rejection::Program { index, code }`;
/// - preflight `BlockhashNotFound` -> transient;
/// - any other preflight transaction error -> rejected,
///   `Rejection::Transaction(error)`;
/// - a JSON-RPC error response without a transaction error from a node that
///   cannot serve the request right now (block not available -32004,
///   `JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE`; node unhealthy -32005,
///   `JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY`) -> transient: the node answered
///   and did not forward the transaction;
/// - any other JSON-RPC error response without a transaction error (for
///   example -32003 signature verification failure, -32602 invalid params
///   such as an oversize transaction) -> rejected, `Rejection::Rpc`: the
///   node refused this transaction and will refuse it again;
/// - transport, IO, timeout, parse and other client errors of the send ->
///   unknown: the request may have reached the node and been forwarded;
/// - client errors raised before the request (local checks of the
///   transaction) -> local.
fn classify_send_error(error: &ClientError) -> SendFailure {
    let ClientError::SolanaRpcTransaction { source, .. } = error else {
        return SendFailure::Local;
    };
    match source.get_transaction_error() {
        Some(TransactionError::InstructionError(index, InstructionError::Custom(code))) => {
            SendFailure::Rejected(Rejection::Program { index, code })
        }
        Some(TransactionError::BlockhashNotFound) => SendFailure::Transient,
        Some(error) => SendFailure::Rejected(Rejection::Transaction(error)),
        None => match source.kind() {
            ErrorKind::RpcError(RpcError::RpcResponseError {
                code:
                    JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE | JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY,
                ..
            }) => SendFailure::Transient,
            ErrorKind::RpcError(RpcError::RpcResponseError { code, message, .. }) => {
                SendFailure::Rejected(Rejection::Rpc {
                    code: *code,
                    message: message.clone(),
                })
            }
            _ => SendFailure::Unknown,
        },
    }
}

/// The status of a step from the statuses of its `sends` (same order) at
/// `block_height`: confirmed if any send succeeded at `confirmed`, else
/// failed if any failed at `confirmed` (the last one reported), else pending
/// if any landed below `confirmed` (it may still be confirmed, or dropped
/// with its fork), else expired if every send's blockhash is past, else
/// pending. A landed success wins over a failure because only one of the
/// sends can spend the inputs. A failure counts only at `confirmed`, like a
/// success: a failure on a minority fork must not re-prove or requeue the
/// step while the same signature can still land on the main fork.
///
/// Ordering invariant: `block_height` must be read before `statuses` (see
/// `poll_sent`). A send missing from `statuses` was then not landed at a
/// moment when the chain was already at `block_height` or above; if
/// `block_height` is past the send's last valid block height, it can never
/// land, so `Expired` is final. Read in the other order, the transaction
/// could land between the two reads and be wrongly reported `Expired`.
pub fn classify(
    sends: &[Sent],
    statuses: &[Option<TransactionStatus>],
    block_height: u64,
) -> StepStatus {
    let mut failure = None;
    let mut unconfirmed = false;
    let landed = sends
        .iter()
        .zip(statuses)
        .filter_map(|(sent, status)| Some((sent, status.as_ref()?)));
    for (sent, status) in landed {
        if !status.satisfies_commitment(CommitmentConfig::confirmed()) {
            unconfirmed = true;
            continue;
        }
        let Some(error) = &status.err else {
            return StepStatus::Confirmed {
                signature: sent.signature,
            };
        };
        failure = Some(StepStatus::Failed {
            reason: error.to_string(),
            custom: CustomError::of(error),
        });
    }
    if let Some(failure) = failure {
        return failure;
    }
    let last_valid = sends
        .iter()
        .map(|sent| sent.last_valid_block_height)
        .max()
        .unwrap_or(0);
    if unconfirmed || block_height <= last_valid {
        StepStatus::Pending
    } else {
        StepStatus::Expired
    }
}

impl Coordinator {
    /// Step `id`'s proven `transact` followed by its tail, with the step's
    /// compute units; `None` before it is proven.
    pub fn send_request(&self, id: StepId) -> Option<SendRequest> {
        let step = self.steps.get(id)?;
        let instruction = step.instruction.clone()?;
        Some(SendRequest {
            instructions: std::iter::once(instruction)
                .chain(step.tail.iter().cloned())
                .collect(),
            compute_units: step.compute_units(),
        })
    }

    /// Spawns a send for every proven non-fill step without a pending retry,
    /// unless shutting down.
    pub fn send_ready(&mut self) {
        if self.runtime.cancel.is_cancelled() {
            return;
        }
        for id in self.steps.ready_to_send() {
            self.spawn_send(id);
        }
    }

    /// Sends step `id` in a spawned task and reports `Event::Sent`. A settled
    /// fill resubmits its co-signed transaction unchanged (the user's
    /// signature binds its blockhash); other steps are compiled anew.
    pub fn spawn_send(&mut self, id: StepId) {
        let signed = self.steps.get(id).and_then(|step| {
            let fill = step.fill.as_ref()?;
            Some(SendJob::Resubmit(
                fill.transaction.clone()?,
                fill.last_valid_block_height,
            ))
        });
        let Some(job) = signed.or_else(|| self.send_request(id).map(SendJob::Compile)) else {
            return;
        };
        if let Some(step) = self.steps.get_mut(id) {
            step.state = StepState::Sending;
        }
        let sender = self.services.sender.clone();
        let events = self.runtime.events.clone();
        self.runtime.tasks.spawn(async move {
            let outcome = match job {
                SendJob::Resubmit(transaction, last_valid) => {
                    sender.submit(&transaction, last_valid).await
                }
                SendJob::Compile(request) => sender.send(&request).await,
            };
            let _ = events.send(Event::Sent { step: id, outcome });
        });
    }

    /// Re-emits `Event::Send(id)` after one `status_interval`; exits without
    /// sending when the coordinator is cancelled first.
    fn spawn_send_retry(&self, id: StepId) {
        let events = self.runtime.events.clone();
        let cancel = self.runtime.cancel.clone();
        let backoff = self.config.status_interval;
        self.runtime.tasks.spawn(async move {
            if cancel
                .run_until_cancelled(tokio::time::sleep(backoff))
                .await
                .is_some()
            {
                let _ = events.send(Event::Send(id));
            }
        });
    }

    /// Sends a step again after a backoff retry, if it is still proven and
    /// the coordinator is not shutting down. A fill whose deadline
    /// (`FillTransfer::expires_at`) passed while the retry waited is not
    /// sent: the retry reports it as `NotSent` again through `Event::Sent`,
    /// whose handler (`on_sent`) aborts it on the coordinator loop.
    pub fn on_send_retry(&mut self, id: StepId) {
        let Some(step) = self.steps.get(id) else {
            return;
        };
        if step.state != StepState::Proven || self.runtime.cancel.is_cancelled() {
            return;
        }
        if fill_deadline_passed(step, Instant::now()) {
            let _ = self.runtime.events.send(Event::Sent {
                step: id,
                outcome: SendOutcome::NotSent(MarketMakerError::ReservationExpired { step: id }),
            });
            return;
        }
        self.spawn_send(id);
    }

    /// Handles the result of a send:
    /// - `Sent` and `OutcomeUnknown` record the signature and poll it.
    /// - `NotSent` is a transient failure (stale blockhash, unhealthy node,
    ///   failed blockhash fetch or signing): the step goes back to proven and
    ///   is sent again after one `status_interval` of backoff, for every step
    ///   kind. After `SEND_ATTEMPTS` consecutive failures the step aborts,
    ///   failing a fill and requeueing any other operation. A fill is also
    ///   failed at once, with `MarketMakerError::ReservationExpired`, when its
    ///   deadline (`FillTransfer::expires_at`) has passed or passes before
    ///   the backoff retry would send it; `on_send_retry` checks the deadline
    ///   again. Shutdown cancels pending retries.
    /// - `Rejected` with no earlier sends goes through `handle_failure`, the
    ///   same path as a transaction that landed and failed: a fill whose
    ///   market maker transfer failed with `NullifierAlreadyQueued`
    ///   (`CustomError::is_order_already_filled`) fails with
    ///   `SwapError::OrderAlreadyFilled`; a non-fill step re-proves on a
    ///   stale-root or proof-verification code (preflight runs against the
    ///   current roots, which can rotate between proving and sending, so a
    ///   preflight failure is the common form of a stale proof); any other
    ///   rejection requeues the operation, or fails it for a fill, whose user
    ///   signature binds the transaction.
    /// - `Rejected` after earlier sends keeps polling those signatures; one of
    ///   them may still land.
    pub async fn on_sent(&mut self, id: StepId, outcome: SendOutcome) {
        let backoff = self.config.status_interval;
        let Some(step) = self.steps.get_mut(id) else {
            return;
        };
        if !matches!(outcome, SendOutcome::NotSent(_)) {
            step.send_failures = 0;
        }
        match outcome {
            SendOutcome::Sent(sent) => {
                step.sends.push(sent);
                step.state = StepState::Sent;
            }
            SendOutcome::OutcomeUnknown { sent, error } => {
                tracing::warn!(step = id, %error, "send outcome unknown, polling its signature");
                step.sends.push(sent);
                step.state = StepState::Sent;
            }
            SendOutcome::NotSent(error) => {
                step.state = StepState::Proven;
                step.send_failures = step.send_failures.saturating_add(1);
                let retry_at = Instant::now().checked_add(backoff);
                let expired = retry_at.is_none_or(|retry_at| fill_deadline_passed(step, retry_at));
                if step.kind == StepKind::Fill && expired {
                    tracing::warn!(step = id, %error, "fill was not sent before its deadline");
                    self.abort(
                        id,
                        MarketMakerError::ReservationExpired { step: id },
                        Retry::Fail,
                    )
                    .await;
                    return;
                }
                if step.send_failures >= SEND_ATTEMPTS {
                    let retry = step.kind.retry();
                    self.abort(id, error, retry).await;
                    return;
                }
                tracing::warn!(
                    step = id,
                    attempt = step.send_failures,
                    %error,
                    "step was not sent, retrying after backoff"
                );
                self.spawn_send_retry(id);
            }
            SendOutcome::Rejected(rejection) if step.sends.is_empty() => {
                let custom = rejection.custom_error();
                self.handle_failure(id, custom, rejection.into()).await;
            }
            SendOutcome::Rejected(rejection) => {
                let error = MarketMakerError::from(rejection);
                tracing::warn!(step = id, %error, "resend rejected, polling earlier signatures");
                step.resend_failed = true;
                step.state = StepState::Sent;
            }
        }
    }
}

/// Whether `step` is a fill whose deadline (`FillTransfer::expires_at`) is
/// at or before `at`. A fill without a deadline yet, or any other step, has
/// none to pass.
fn fill_deadline_passed(step: &Step, at: Instant) -> bool {
    step.fill
        .as_ref()
        .and_then(|fill| fill.expires_at)
        .is_some_and(|deadline| at >= deadline)
}

#[cfg(test)]
mod tests {
    use solana_rpc_client_api::{
        client_error::Error as RpcClientError, request::RpcResponseErrorData,
        response::RpcSimulateTransactionResult,
    };
    use solana_transaction_status_client_types::TransactionConfirmationStatus;

    use super::*;

    fn send_error(kind: ErrorKind) -> ClientError {
        ClientError::SolanaRpcTransaction {
            operation: "send_transaction_with_config",
            source: RpcClientError::from(kind),
        }
    }

    fn response_error(code: i64, error_data: RpcResponseErrorData) -> ClientError {
        send_error(ErrorKind::RpcError(RpcError::RpcResponseError {
            code,
            message: String::new(),
            data: error_data,
        }))
    }

    fn preflight_failure(error: TransactionError) -> ClientError {
        response_error(
            -32002,
            RpcResponseErrorData::SendTransactionPreflightFailure(RpcSimulateTransactionResult {
                err: Some(error.into()),
                logs: None,
                accounts: None,
                units_consumed: None,
                loaded_accounts_data_size: None,
                return_data: None,
                inner_instructions: None,
                replacement_blockhash: None,
                fee: None,
                pre_balances: None,
                post_balances: None,
                pre_token_balances: None,
                post_token_balances: None,
                loaded_addresses: None,
            }),
        )
    }

    /// Each kind of send failure maps to its handling: a preflight program
    /// or transaction error is a permanent rejection, a stale blockhash or an
    /// unhealthy node is transient, any other JSON-RPC error without a
    /// transaction error is a permanent rpc rejection, a transport error
    /// leaves the outcome unknown, and a client-side error is local.
    #[test]
    fn classify_send_error_maps_each_failure() {
        type IsWant = fn(&SendFailure) -> bool;
        let cases: [(&str, ClientError, &str, IsWant); 9] = [
            (
                "preflight custom program error",
                preflight_failure(TransactionError::InstructionError(
                    1,
                    InstructionError::Custom(7015),
                )),
                "Rejected(Program { index: 1, code: 7015 })",
                |failure| {
                    matches!(
                        failure,
                        SendFailure::Rejected(Rejection::Program {
                            index: 1,
                            code: 7015
                        })
                    )
                },
            ),
            (
                "preflight blockhash not found",
                preflight_failure(TransactionError::BlockhashNotFound),
                "Transient",
                |failure| matches!(failure, SendFailure::Transient),
            ),
            (
                "preflight transaction error",
                preflight_failure(TransactionError::AccountNotFound),
                "Rejected(Transaction(AccountNotFound))",
                |failure| {
                    matches!(
                        failure,
                        SendFailure::Rejected(Rejection::Transaction(
                            TransactionError::AccountNotFound
                        ))
                    )
                },
            ),
            (
                "node unhealthy (-32005)",
                response_error(
                    -32005,
                    RpcResponseErrorData::NodeUnhealthy {
                        num_slots_behind: Some(10),
                    },
                ),
                "Transient",
                |failure| matches!(failure, SendFailure::Transient),
            ),
            (
                "block not available (-32004)",
                response_error(-32004, RpcResponseErrorData::Empty),
                "Transient",
                |failure| matches!(failure, SendFailure::Transient),
            ),
            (
                "signature verification failure (-32003)",
                response_error(-32003, RpcResponseErrorData::Empty),
                "Rejected(Rpc { code: -32003 })",
                |failure| {
                    matches!(
                        failure,
                        SendFailure::Rejected(Rejection::Rpc { code: -32003, .. })
                    )
                },
            ),
            (
                "invalid params (-32602)",
                response_error(-32602, RpcResponseErrorData::Empty),
                "Rejected(Rpc { code: -32602 })",
                |failure| {
                    matches!(
                        failure,
                        SendFailure::Rejected(Rejection::Rpc { code: -32602, .. })
                    )
                },
            ),
            (
                "io timeout",
                send_error(ErrorKind::Io(std::io::Error::from(
                    std::io::ErrorKind::TimedOut,
                ))),
                "Unknown",
                |failure| matches!(failure, SendFailure::Unknown),
            ),
            (
                "client-side error",
                ClientError::MissingOutput,
                "Local",
                |failure| matches!(failure, SendFailure::Local),
            ),
        ];
        for (label, error, want, is_want) in cases {
            let failure = classify_send_error(&error);
            assert!(is_want(&failure), "{label}: got {failure:?}, want {want}");
        }
    }

    /// A program rejection exposes its instruction and custom code and
    /// converts to `TransactionRejected` carrying the same instruction error.
    #[test]
    fn program_rejection_becomes_transaction_rejected() {
        let rejection = Rejection::Program { index: 2, code: 0 };
        assert_eq!(
            rejection.custom_error(),
            Some(CustomError { index: 2, code: 0 }),
            "custom error"
        );
        let converted = MarketMakerError::from(rejection);
        assert!(
            matches!(
                converted,
                MarketMakerError::TransactionRejected {
                    error: TransactionError::InstructionError(2, InstructionError::Custom(0))
                }
            ),
            "got {converted:?}, want TransactionRejected(InstructionError(2, Custom(0)))"
        );
    }

    /// An rpc rejection has no custom error and converts to `RpcRejected`
    /// with the same code and message.
    #[test]
    fn rpc_rejection_becomes_rpc_rejected() {
        let rejection = Rejection::Rpc {
            code: -32003,
            message: "signature verification failure".to_string(),
        };
        assert_eq!(rejection.custom_error(), None, "custom error");
        let converted = MarketMakerError::from(rejection);
        assert!(
            matches!(
                &converted,
                MarketMakerError::RpcRejected { code: -32003, message }
                    if message == "signature verification failure"
            ),
            "got {converted:?}, want RpcRejected(-32003)"
        );
    }

    fn sent(last_valid_block_height: u64) -> Sent {
        Sent {
            signature: Signature::default(),
            last_valid_block_height,
        }
    }

    fn status(
        err: Option<TransactionError>,
        confirmation_status: TransactionConfirmationStatus,
    ) -> TransactionStatus {
        TransactionStatus {
            slot: 1,
            confirmations: Some(1),
            status: err.clone().map_or(Ok(()), Err),
            err,
            confirmation_status: Some(confirmation_status),
        }
    }

    /// A duplicate fill that lands and fails reports the market maker
    /// transfer's `NullifierAlreadyQueued`, which maps to an already filled
    /// order for a fill step only, exactly like the preflight rejection.
    #[test]
    fn landed_duplicate_address_is_order_already_filled() {
        let queued = ShieldedPoolError::NullifierAlreadyQueued as u32;
        let duplicate = TransactionError::InstructionError(
            MARKET_MAKER_TRANSFER_INDEX,
            InstructionError::Custom(queued),
        );
        let sends = [sent(100)];
        let statuses = [Some(status(
            Some(duplicate.clone()),
            TransactionConfirmationStatus::Confirmed,
        ))];
        let classified = classify(&sends, &statuses, 50);
        let want_custom = CustomError {
            index: MARKET_MAKER_TRANSFER_INDEX,
            code: queued,
        };
        assert_eq!(
            classified,
            StepStatus::Failed {
                reason: duplicate.to_string(),
                custom: Some(want_custom),
            },
            "landed duplicate order address"
        );
        let preflight = Rejection::Program {
            index: MARKET_MAKER_TRANSFER_INDEX,
            code: queued,
        }
        .custom_error();
        assert_eq!(
            preflight,
            Some(want_custom),
            "preflight duplicate order address"
        );
        let cases = [
            (want_custom, StepKind::Fill, true),
            (want_custom, StepKind::Consolidate, false),
            (want_custom, StepKind::Rebalance, false),
            (
                CustomError {
                    index: MARKET_MAKER_TRANSFER_INDEX.wrapping_sub(1),
                    code: queued,
                },
                StepKind::Fill,
                false,
            ),
            (
                CustomError {
                    index: MARKET_MAKER_TRANSFER_INDEX,
                    code: queued.wrapping_add(1),
                },
                StepKind::Fill,
                false,
            ),
        ];
        for (custom, kind, want) in cases {
            assert_eq!(
                custom.is_order_already_filled(kind),
                want,
                "{custom:?} for {kind:?}"
            );
        }
    }

    /// A status counts only at `confirmed`: a failure or success seen at
    /// `processed` keeps the step pending, also past its blockhash, and a
    /// missing status past every last valid block height is expired.
    #[test]
    fn classify_requires_confirmed_commitment() {
        let failure = TransactionError::InstructionError(0, InstructionError::Custom(7015));
        let cases = [
            (
                "failure at processed",
                Some(status(
                    Some(failure.clone()),
                    TransactionConfirmationStatus::Processed,
                )),
                50,
                StepStatus::Pending,
            ),
            (
                "success at processed past its blockhash",
                Some(status(None, TransactionConfirmationStatus::Processed)),
                150,
                StepStatus::Pending,
            ),
            (
                "failure at confirmed",
                Some(status(
                    Some(failure.clone()),
                    TransactionConfirmationStatus::Confirmed,
                )),
                50,
                StepStatus::Failed {
                    reason: failure.to_string(),
                    custom: Some(CustomError {
                        index: 0,
                        code: 7015,
                    }),
                },
            ),
            (
                "missing within its blockhash",
                None,
                100,
                StepStatus::Pending,
            ),
            ("missing past its blockhash", None, 101, StepStatus::Expired),
        ];
        let sends = [sent(100)];
        for (label, status, block_height, want) in cases {
            assert_eq!(classify(&sends, &[status], block_height), want, "{label}");
        }
    }
}

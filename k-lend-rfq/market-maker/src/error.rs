//! The market maker's error type. Swap checks keep their `SwapError` inside
//! `MarketMakerError::Swap`, so callers can match the exact check that failed.

use k_lend_rfq_sdk::swap::SwapError;
use solana_address::Address;
use solana_signature::Signature;
use solana_signer::SignerError;
use thiserror::Error;
use zolana_client::ClientError;
use zolana_keypair::KeypairError;
use zolana_transaction::TransactionError;

use crate::{
    config::ConfigError,
    transactions::{budget::BudgetError, steps::StepId},
};

/// Every failure of a market maker operation.
#[derive(Debug, Error)]
pub enum MarketMakerError {
    /// Queuing an operation found its asset's unreserved balance, plus the
    /// change of in-flight steps, minus queued amounts, short.
    #[error("{asset} balance {available} minus queued operations cannot cover {requested}")]
    InsufficientBalance {
        asset: Address,
        available: u64,
        requested: u64,
    },

    /// The balance covers the amount, but not within one transaction's
    /// input count.
    #[error("{asset} utxos hold {available} but no {max_inputs} of them cover {requested}")]
    FragmentedInventory {
        asset: Address,
        available: u64,
        requested: u64,
        max_inputs: usize,
    },

    /// A transfer plan's change parts do not sum to the change it keeps.
    #[error("own outputs of {planned} do not add up to the {own_value} the transfer keeps")]
    OwnPartsMismatch { planned: u64, own_value: u64 },

    #[error("{asset} has no fragments to consolidate")]
    NothingToConsolidate { asset: Address },

    #[error("vault {vault} does not exist")]
    VaultMissing { vault: Address },

    #[error("vault {vault} state does not parse: {reason}")]
    VaultState { vault: Address, reason: String },

    #[error("vault {vault} instruction cannot be built: {reason}")]
    VaultInstruction { vault: Address, reason: String },

    #[error("vault {vault} cannot price the rebalance: {reason}")]
    VaultMath { vault: Address, reason: String },

    #[error("shield instruction could not be built: {0}")]
    ShieldInstruction(String),

    #[error("token account {account} does not parse")]
    TokenAccount { account: Address },

    #[error("the simulated kVault operation leaves no {asset} to shield in {account}")]
    NothingToShield { asset: Address, account: Address },

    #[error("simulation failed: {error}")]
    SimulationFailed { error: String },

    #[error("operation amount is zero")]
    AmountZero,

    #[error("amount overflow in {context}")]
    AmountOverflow { context: &'static str },

    /// An instant computed from a configured duration overflows the clock.
    #[error("deadline overflow in {context}")]
    DeadlineOverflow { context: &'static str },

    /// A requeued operation hit its attempt limit; `reason` is the last error.
    #[error("operation failed after {attempts} attempts: {reason}")]
    OperationFailed { attempts: u32, reason: String },

    /// Inputs of a failed step are spent on chain, so it is not retried.
    #[error(
        "{count} inputs of the operation were spent on-chain; the transaction or another spender consumed them"
    )]
    InputsSpent { count: usize },

    #[error("the market maker is shutting down")]
    ShuttingDown,

    #[error("the coordinator stopped")]
    CoordinatorStopped,

    #[error("signer {pubkey} failed: {source}")]
    Signer {
        pubkey: Address,
        source: SignerError,
    },

    #[error("the swap transaction names {signer} as a signer the market maker cannot sign for")]
    UnexpectedSigner { signer: Address },

    #[error("message requires {required} signatures but names {accounts} accounts")]
    MalformedMessage { required: usize, accounts: usize },

    #[error("the market maker is not a signer of the message")]
    MarketMakerNotSigner,

    #[error("the message has no user signer")]
    MissingUserSigner,

    #[error("signature does not verify for user {signer}")]
    InvalidUserSignature { signer: Address },

    #[error("transaction carries no signature")]
    MissingSignature,

    #[error("rpc error: {0}")]
    Rpc(ClientError),

    #[error("indexer error: {0}")]
    Indexer(ClientError),

    #[error("prover error: {0}")]
    Prover(ClientError),

    /// Assembling a fill's order address slot failed
    /// (`k_lend_rfq_sdk::address::add_order_address`): the address does not
    /// derive, no padding slot, a missing or mismatched address proof, or
    /// zolana's assembly error.
    #[error("order address slot: {0}")]
    AddressSlot(String),

    #[error("wallet sync failed: {0}")]
    Sync(ClientError),

    #[error(transparent)]
    Client(#[from] ClientError),

    #[error(transparent)]
    Transaction(#[from] TransactionError),

    #[error(transparent)]
    Keypair(#[from] KeypairError),

    #[error(transparent)]
    Budget(#[from] BudgetError),

    #[error("transaction rejected: {error}")]
    TransactionRejected {
        error: solana_transaction_error::TransactionError,
    },

    /// The rpc refused the transaction with a JSON-RPC error that carries no
    /// transaction error and does not clear on a retry (for example -32003
    /// signature verification or -32602 invalid params).
    #[error("rpc rejected the transaction with error {code}: {message}")]
    RpcRejected { code: i64, message: String },

    #[error("the transaction landed and failed: {0}")]
    TransactionFailed(String),

    /// Every send's blockhash expired without the transaction landing.
    #[error("the transaction did not land before its blockhash expired")]
    NotLanded,

    /// An unsent upkeep step was discarded to free UTXOs for a fill.
    #[error("a fill preempts this upkeep step")]
    Preempted,

    #[error("output position {position} does not fit a slot index")]
    OutputPositionOutOfRange { position: usize },

    #[error("utxo {0:?} is reserved by another step")]
    UtxoReserved([u8; 32]),

    #[error("utxo {0:?} is not tracked")]
    UtxoNotTracked([u8; 32]),

    /// No fill step awaits a signature for the submitted message.
    #[error("the fill is not reserved")]
    UnknownFill,

    /// The fill's deadline passed, or its `fill` caller went away, before
    /// the user signed; its reservations were released.
    #[error("fill {step} was released at its quote deadline")]
    ReservationExpired { step: StepId },

    #[error("fill {step} is already settling")]
    AlreadySettling { step: StepId },

    #[error("transaction {signature} was not confirmed")]
    NotConfirmed { signature: Signature },

    #[error("transaction {signature} was confirmed but not indexed")]
    NotIndexed { signature: Signature },

    #[error("mint {mint} has no shielded-pool asset registry")]
    AssetNotRegistered { mint: Address },

    #[error("asset registry of mint {mint} does not parse: {reason}")]
    AssetRegistry { mint: Address, reason: String },

    #[error("pair of vault {vault} is not served")]
    PairNotServed { vault: Address },

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("a {expected} operation returned another outcome")]
    UnexpectedOutcome { expected: &'static str },

    #[error(transparent)]
    Swap(#[from] SwapError),

    /// The `spawn_blocking` task that builds and encrypts a transfer panicked
    /// or was cancelled.
    #[error("blocking task failed: {0}")]
    BlockingTask(String),
}

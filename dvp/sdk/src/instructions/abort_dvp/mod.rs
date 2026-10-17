mod transaction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortMode {
    Cancel,
    Reject,
}
pub use transaction::{AbortDvp, AbortDvpTransaction, AbortDvpTransactionParams};

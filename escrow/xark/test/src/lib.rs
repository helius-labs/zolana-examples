//! Localnet end-to-end tests and the CU benchmark of the timelock escrow
//! program live in `tests/`.

use anyhow::Result;
use solana_signature::Signature;
use timelock_escrow_program::error::TimelockEscrowError;

/// Asserts that `result` failed with `error`.
#[track_caller]
pub fn assert_escrow_error(context: &str, result: Result<Signature>, error: TimelockEscrowError) {
    let text = failure_text(context, result);
    assert!(
        mentions(&text, error),
        "{context}: expected {error:?} ({}) in: {text}",
        error as u32
    );
}

/// The error text of `result`, which must have failed.
#[track_caller]
fn failure_text(context: &str, result: Result<Signature>) -> String {
    match result {
        Ok(signature) => panic!("{context}: unexpectedly succeeded ({signature})"),
        Err(err) => format!("{err:?}"),
    }
}

/// A custom program error surfaces as `Custom(<decimal>)` or as the
/// program-log line `custom program error: 0x<hex>`; match only those
/// delimited forms.
fn mentions(text: &str, error: TimelockEscrowError) -> bool {
    let code = error as u32;
    let hex = format!("0x{code:x}");
    text.contains(&format!("Custom({code})"))
        || text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == hex)
}

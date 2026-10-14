//! Localnet end-to-end tests for the DvP program live in `tests/`.

use anyhow::{anyhow, Result};
use dvp_program::error::DvpError;
use dvp_sdk::{
    index::DiscoveredDvp,
    state::{Deposit, Leg, SwapDvp},
};
use solana_clock::Clock;
use solana_signature::Signature;
use zolana_client::Rpc;

/// Every DvP program error, in code order.
const DVP_ERRORS: [DvpError; 7] = [
    DvpError::ProofVerificationFailed,
    DvpError::InvalidInstructionData,
    DvpError::InvalidShieldedPoolProgram,
    DvpError::MissingDvpAuthority,
    DvpError::InvalidDvpAuthority,
    DvpError::HashingFailed,
    DvpError::TimestampOutOfWindow,
];

/// A settle `current_timestamp`: the cluster clock's `unix_timestamp`, read
/// from the Clock sysvar account, minus 2 seconds. The program refuses a
/// timestamp ahead of its clock, so the lag absorbs a transaction landing on
/// a clock that reads behind the sampled one. It spends that much of
/// `TIME_WINDOW`, which then has to cover proving and sending.
pub fn settle_timestamp(client: &impl Rpc) -> Result<i64> {
    let account = client
        .get_account(solana_clock::sysvar::ID)?
        .ok_or_else(|| anyhow!("no Clock sysvar account"))?;
    let clock: Clock = wincode::deserialize(&account.data)?;
    Ok(clock.unix_timestamp - 2)
}

/// `index_terms` readiness: the DvP of `terms` is among those found.
pub fn has_dvp(terms: &SwapDvp) -> impl Fn(&[DiscoveredDvp]) -> bool + '_ {
    move |found| found.iter().any(|dvp| &dvp.dvp_utxo.terms == terms)
}

/// Deposit-discovery readiness: every leg of `legs` has a deposit.
pub fn has_legs<const N: usize>(legs: [Leg; N]) -> impl Fn(&[Deposit]) -> bool {
    move |found| {
        legs.iter()
            .all(|leg| found.iter().any(|deposit| deposit.leg == *leg))
    }
}

/// Asserts that `result` failed with `error`.
#[track_caller]
pub fn assert_dvp_error(context: &str, result: Result<Signature>, error: DvpError) {
    let text = failure_text(context, result);
    assert!(
        mentions(&text, error),
        "{context}: expected {error:?} ({}) in: {text}",
        error as u32
    );
}

/// Asserts that `result` failed past the DvP program, with no DvP error: the
/// replay negatives expect SPP to refuse a nullifier that already exists,
/// not a proof, a stale timestamp or any other DvP check.
#[track_caller]
pub fn assert_spp_rejects(context: &str, result: Result<Signature>) {
    let text = failure_text(context, result);
    if let Some(error) = DVP_ERRORS.into_iter().find(|error| mentions(&text, *error)) {
        panic!("{context}: failed with {error:?} instead of in SPP: {text}");
    }
}

/// The error text of `result`, which must have failed.
#[track_caller]
pub fn failure_text(context: &str, result: Result<Signature>) -> String {
    match result {
        Ok(signature) => panic!("{context}: unexpectedly succeeded ({signature})"),
        Err(err) => format!("{err:?}"),
    }
}

/// A custom program error surfaces as `Custom(<decimal>)` or as the
/// program-log line `custom program error: 0x<hex>`; match only those
/// delimited forms.
fn mentions(text: &str, error: DvpError) -> bool {
    let code = error as u32;
    let hex = format!("0x{code:x}");
    text.contains(&format!("Custom({code})"))
        || text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == hex)
}

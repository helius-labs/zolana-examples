//! Assertions on the exact error a market maker or user call fails with.

use anyhow::Result;
use k_lend_market_maker::MakerError;
use k_lend_rfq_sdk::swap::SwapError;

/// Asserts that `call` failed with a [`SwapError`] for which `is_want`
/// holds. `want` names the expected variant in the failure message
/// `got {err:?}, want {want}`.
#[track_caller]
pub fn assert_swap_error<T>(call: Result<T>, want: &str, is_want: impl FnOnce(&SwapError) -> bool) {
    assert_error(call, want, is_want);
}

/// Asserts that `call` failed with a [`MakerError`] for which `is_want`
/// holds. `want` names the expected variant in the failure message
/// `got {err:?}, want {want}`.
#[track_caller]
pub fn assert_maker_error<T>(
    call: Result<T>,
    want: &str,
    is_want: impl FnOnce(&MakerError) -> bool,
) {
    assert_error(call, want, is_want);
}

#[track_caller]
fn assert_error<T, E>(call: Result<T>, want: &str, is_want: impl FnOnce(&E) -> bool)
where
    E: std::fmt::Display + std::fmt::Debug + Send + Sync + 'static,
{
    match call {
        Ok(_) => panic!("got Ok, want {want}"),
        Err(err) => assert!(
            err.downcast_ref::<E>().is_some_and(is_want),
            "got {err:?}, want {want}"
        ),
    }
}

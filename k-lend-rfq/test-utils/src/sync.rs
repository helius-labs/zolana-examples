use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::sync::Barrier;

/// Longest a test task waits at a barrier. A task that errors before reaching
/// the barrier never arrives, so the others fail after this instead of hanging
/// until the CI job timeout.
pub const BARRIER_TIMEOUT: Duration = Duration::from_secs(120);

/// Waits at `barrier` for at most `BARRIER_TIMEOUT`; on timeout returns
/// `timed out waiting for {what}`, naming the rendezvous that was missed.
pub async fn wait_or_timeout(barrier: &Barrier, what: &str) -> Result<()> {
    tokio::time::timeout(BARRIER_TIMEOUT, barrier.wait())
        .await
        .map(|_| ())
        .map_err(|_| anyhow!("timed out waiting for {what}"))
}

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use zolana_client::Rpc;
use zolana_transaction::ShieldedTransaction;

use crate::err;

const INDEX_POLL: Duration = Duration::from_millis(500);

/// Polls `collect` until it finds something or `timeout` passes.
pub(crate) fn index_until<T>(
    timeout: Duration,
    what: &str,
    mut collect: impl FnMut() -> Result<Vec<T>>,
) -> Result<Vec<T>> {
    let deadline = Instant::now() + timeout;
    loop {
        let found = collect()?;
        if !found.is_empty() {
            return Ok(found);
        }
        if Instant::now() >= deadline {
            bail!("timed out discovering {what}");
        }
        std::thread::sleep(INDEX_POLL);
    }
}

/// Runs `scan` over every indexed transaction with an output or a message
/// tagged `tag`, page by page, and collects what it returns.
pub(crate) fn collect_tagged<I: Rpc, T>(
    tag: [u8; 32],
    indexer: &I,
    mut scan: impl FnMut(&ShieldedTransaction) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let mut found = Vec::new();
    let mut cursor = None;
    loop {
        let page = indexer
            .get_shielded_transactions_by_tags(vec![tag], cursor, None, None)
            .map_err(err)?;
        for tx in &page.transactions {
            found.extend(scan(tx)?);
        }
        let Some(next) = page.next_cursor else {
            return Ok(found);
        };
        cursor = Some(next);
    }
}

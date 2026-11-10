use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use spr_client::{HistoryFilters, Rpc};
use spr_transaction::ShieldedTransaction;

use crate::err;

const INDEX_POLL: Duration = Duration::from_millis(500);

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

/// Scans every indexed transaction with an output or message under `tag`.
pub(crate) fn collect_tagged<I: Rpc, T>(
    tag: [u8; 32],
    indexer: &I,
    mut scan: impl FnMut(&ShieldedTransaction) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let mut found = Vec::new();
    let mut cursor = None;
    loop {
        let page = indexer
            .get_shielded_transactions_by_tags(
                vec![tag],
                cursor,
                None,
                HistoryFilters::default(),
                None,
            )
            .map_err(err)?;
        for tx in page.data.iter().flat_map(|group| &group.events) {
            if let Some(item) = scan(tx)? {
                found.push(item);
            }
        }
        cursor = page.continuation(cursor).map_err(err)?;
        if cursor.is_none() {
            return Ok(found);
        }
    }
}

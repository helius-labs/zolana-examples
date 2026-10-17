use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use zolana_client::Rpc;
use zolana_transaction::ShieldedTransaction;

use crate::err;

const INDEX_POLL: Duration = Duration::from_millis(500);

/// Indexer page size for the nullifier queries.
const PAGE_LIMIT: u32 = 100;

/// Polls `collect` until what it returns satisfies `ready` or `timeout`
/// passes. An indexer error does not end the poll: the next round retries
/// it, and a timeout reports the last error.
pub(crate) fn index_until<T>(
    timeout: Duration,
    what: &str,
    mut collect: impl FnMut() -> Result<Vec<T>>,
    ready: impl Fn(&[T]) -> bool,
) -> Result<Vec<T>> {
    let start = Instant::now();
    loop {
        let last_error = match collect() {
            Ok(found) if ready(&found) => return Ok(found),
            Ok(_) => None,
            Err(error) => Some(error),
        };
        if start.elapsed() >= timeout {
            return Err(match last_error {
                Some(error) => error.context(format!("timed out discovering {what}")),
                None => anyhow!("timed out discovering {what}"),
            });
        }
        std::thread::sleep(INDEX_POLL);
    }
}

/// Runs `scan` over every indexed transaction with an output or a message
/// tagged `tag`, page by page, and collects what it returns. A transaction
/// served on two pages is scanned once; it is keyed by signature and event
/// index, since one Solana transaction can emit several.
pub(crate) fn collect_tagged<I: Rpc, F: IntoIterator>(
    tag: [u8; 32],
    indexer: &I,
    mut scan: impl FnMut(&ShieldedTransaction) -> Result<F>,
) -> Result<Vec<F::Item>> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = None;
    loop {
        let page = indexer
            .get_shielded_transactions_by_tags(vec![tag], cursor, None, None)
            .map_err(err)?;
        for tx in &page.transactions {
            if seen.insert((tx.tx_signature, tx.event_index)) {
                found.extend(scan(tx)?);
            }
        }
        let Some(next) = page.next_cursor else {
            return Ok(found);
        };
        cursor = Some(next);
    }
}

/// The nullifiers among `nullifiers` that an indexed transaction spent.
pub(crate) fn spent_nullifiers<I: Rpc>(
    indexer: &I,
    nullifiers: &[[u8; 32]],
) -> Result<HashSet<[u8; 32]>> {
    let wanted: HashSet<_> = nullifiers.iter().copied().collect();
    let mut spent = HashSet::new();
    for chunk in nullifiers.chunks(PAGE_LIMIT as usize) {
        let mut cursor = None;
        loop {
            let page = indexer
                .get_shielded_transactions_by_nullifiers(
                    chunk.to_vec(),
                    cursor,
                    Some(PAGE_LIMIT),
                    None,
                )
                .map_err(err)?;
            spent.extend(
                page.transactions
                    .iter()
                    .flat_map(|tx| tx.nullifiers.iter().copied())
                    .filter(|nullifier| wanted.contains(nullifier)),
            );
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some(next);
        }
    }
    Ok(spent)
}

#[cfg(test)]
mod tests {
    use anyhow::bail;

    use super::*;
    use crate::index::fixture::{deposit_fixture, FixtureIndexer};

    #[test]
    fn index_until_retries_errors_until_ready() {
        let mut rounds = 0;
        let found = index_until(
            Duration::from_secs(10),
            "numbers",
            || {
                rounds += 1;
                if rounds == 1 {
                    bail!("indexer down");
                }
                Ok((1..rounds).collect::<Vec<u32>>())
            },
            |found| found.len() >= 2,
        )
        .expect("ready");
        assert_eq!((rounds, found), (3, vec![1, 2]));
    }

    #[test]
    fn index_until_times_out_with_the_last_error() {
        let error = index_until::<u32>(
            Duration::ZERO,
            "numbers",
            || bail!("indexer down"),
            |_| true,
        )
        .expect_err("timeout");
        assert_eq!(
            format!("{error:#}"),
            "timed out discovering numbers: indexer down"
        );
        let error = index_until(Duration::ZERO, "numbers", || Ok(vec![1u32]), |_| false)
            .expect_err("timeout");
        assert_eq!(format!("{error:#}"), "timed out discovering numbers");
    }

    /// The fixture indexer repeats a transaction on the next page; it is
    /// scanned once. Two events of one Solana transaction are both scanned.
    #[test]
    fn collect_tagged_scans_each_event_once() {
        let fixture = deposit_fixture();
        let [delivery_tx, payment_tx] = fixture.txs.clone();
        let tag = fixture.terms.dvp_authority().expect("pda").0.to_bytes();
        let indexer = FixtureIndexer {
            txs: vec![
                ShieldedTransaction {
                    event_index: Some(0),
                    ..delivery_tx
                },
                ShieldedTransaction {
                    event_index: Some(1),
                    ..payment_tx
                },
            ],
        };
        let scanned = collect_tagged(tag, &indexer, |tx| Ok([tx.event_index])).expect("collect");
        assert_eq!(scanned, vec![Some(0), Some(1)]);
    }
}

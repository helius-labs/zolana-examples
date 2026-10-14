//! Localnet end-to-end tests for the DvP program live in `tests/`.

use anyhow::{anyhow, Result};
use solana_address::Address;
use zolana_client::Rpc;

/// The cluster clock's `unix_timestamp`, read from the Clock sysvar account
/// (`slot, epoch_start_timestamp, epoch, leader_schedule_epoch,
/// unix_timestamp`, little-endian).
pub fn cluster_time(client: &impl Rpc) -> Result<i64> {
    let clock: Address = "SysvarC1ock11111111111111111111111111111111".parse()?;
    let account = client
        .get_account(clock)?
        .ok_or_else(|| anyhow!("no Clock sysvar account"))?;
    let unix_timestamp = account
        .data
        .get(32..40)
        .ok_or_else(|| anyhow!("short Clock sysvar account"))?;
    Ok(i64::from_le_bytes(unix_timestamp.try_into()?))
}

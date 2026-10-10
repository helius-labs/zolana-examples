//! Observations of the market maker's inventory and independent reference
//! values the tests compare them with.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use solana_address::Address;
use solana_signature::Signature;
use zolana_client::SolanaRpc;
use zolana_transaction::WalletUtxo;

use k_lend_market_maker::{Holdings, MarketMaker, SHIELD_MARGIN_BPS, SWEEP_CAP_BPS};
use k_lend_rfq_sdk::{
    pair::{Pair, VaultState},
    swap::FULL_BPS,
};

use crate::{chain::public_balances, setup::POLL};

/// The market maker's net balance of each asset of `pair`: reservations plus
/// unindexed inflows minus the outflows of unlanded fills.
pub fn net_holdings(market_maker: &MarketMaker, pair: &Pair) -> Holdings {
    Holdings {
        collateral: market_maker.net_balance(&pair.token_mint),
        shares: market_maker.net_balance(&pair.shares_mint),
    }
}

/// The market maker's largest spendable UTXO of `asset` as an input list:
/// one UTXO, or none when it holds none.
pub fn largest_utxo(market_maker: &MarketMaker, asset: &Address) -> Vec<WalletUtxo> {
    market_maker
        .spendable(asset)
        .into_iter()
        .max_by_key(|utxo| utxo.utxo.amount)
        .into_iter()
        .collect()
}

/// The balance of the market maker's public share account: the margin a kVault
/// tail leaves unshielded and the next tail sweeps.
pub fn share_account(rpc: &SolanaRpc, market_maker: &MarketMaker, pair: &Pair) -> Result<u64> {
    let [_, shares] = public_balances(rpc, &market_maker.address(), pair)?;
    Ok(shares)
}

/// The residual a tail shielding a payout of `paid` leaves:
/// `SHIELD_MARGIN_BPS` per whole basis point of `paid`, at least 1.
pub fn shield_margin(paid: u64) -> u64 {
    bps_of(paid, SHIELD_MARGIN_BPS)
}

/// The most of an earlier residual a tail paying `paid` sweeps:
/// `SWEEP_CAP_BPS` per whole basis point of `paid`, at least 1.
pub fn sweep_cap(paid: u64) -> u64 {
    bps_of(paid, SWEEP_CAP_BPS)
}

/// `bps` per whole basis point of `paid`, at least 1. The reference the tests
/// compare the market maker with, so it does not call the market maker's own
/// math.
fn bps_of(paid: u64, bps: u64) -> u64 {
    (paid / FULL_BPS).saturating_mul(bps).max(1)
}

/// The `amount_out` of a deposit quote of `amount_in` at `fee_bps` on an
/// uninvested, fee-free vault, from the plain share math: the vault mints
/// `floor(shares_issued * amount_in / token_available)` shares (one per token
/// into an empty vault) and the user receives
/// `floor(minted * (FULL_BPS - fee_bps) / FULL_BPS)` of them. Fails on a vault
/// this math does not describe.
pub fn plain_deposit_out(vault: &VaultState, amount_in: u64, fee_bps: u64) -> Result<u64> {
    if !vault.reserves.as_slice().is_empty() || vault.pending_fees_sf != 0 || vault.crank_funds != 0
    {
        bail!("the plain share math needs an uninvested, fee-free vault, got {vault:?}");
    }
    let minted = if vault.shares_issued == 0 {
        u128::from(amount_in)
    } else {
        u128::from(vault.shares_issued) * u128::from(amount_in) / u128::from(vault.token_available)
    };
    let kept = FULL_BPS
        .checked_sub(fee_bps)
        .ok_or_else(|| anyhow!("fee of {fee_bps} bps above the whole"))?;
    Ok(u64::try_from(
        minted * u128::from(kept) / u128::from(FULL_BPS),
    )?)
}

/// The first rebalance that landed, polled until `timeout`.
pub async fn wait_for_rebalance(
    market_maker: &MarketMaker,
    timeout: Duration,
) -> Result<Signature> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(signature) = market_maker.rebalances().first() {
            return Ok(*signature);
        }
        if Instant::now() >= deadline {
            bail!("no automatic rebalance after {timeout:?}");
        }
        tokio::time::sleep(POLL).await;
    }
}

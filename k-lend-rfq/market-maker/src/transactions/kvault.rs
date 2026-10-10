//! Async rpc reads of the maker's vault and token accounts.

use solana_address::Address;
use zolana_client::AsyncRpc;

use k_lend_rfq_sdk::{
    kvault::token_account_amount,
    pair::{Pair, VaultState},
};

use crate::{config::check_pair, error::MakerError};

/// The token balance of `account`; a missing account holds 0.
pub async fn token_balance(rpc: &dyn AsyncRpc, account: Address) -> Result<u64, MakerError> {
    match rpc.get_account(account).await.map_err(MakerError::Rpc)? {
        None => Ok(0),
        Some(found) => {
            token_account_amount(&found.data).ok_or(MakerError::TokenAccount { account })
        }
    }
}

/// Reads and prices `vault` over the async rpc: the vault account first (its allocations name the reserves), then
/// its `GlobalConfig` and every allocated reserve in one
/// `get_multiple_accounts`, priced by `VaultState::from_accounts`.
///
/// Errors with `VaultMissing` when the vault does not exist and with
/// `VaultState` when one of the other accounts is missing, an account does
/// not parse or the reserves do not match the vault's allocations.
pub async fn read_vault(rpc: &dyn AsyncRpc, vault: Address) -> Result<VaultState, MakerError> {
    let parse = |error: anyhow::Error| MakerError::VaultState {
        vault,
        reason: error.to_string(),
    };
    let vault_data = rpc
        .get_account(vault)
        .await
        .map_err(MakerError::Rpc)?
        .ok_or(MakerError::VaultMissing { vault })?
        .data;
    let addresses = VaultState::pricing_accounts(&vault_data).map_err(parse)?;
    let fetched = rpc
        .get_multiple_accounts(addresses.clone())
        .await
        .map_err(MakerError::Rpc)?;
    if fetched.len() != addresses.len() {
        return Err(MakerError::VaultState {
            vault,
            reason: format!(
                "{} of {} pricing accounts returned",
                fetched.len(),
                addresses.len()
            ),
        });
    }
    let accounts = addresses
        .into_iter()
        .zip(fetched)
        .map(|(address, account)| {
            account
                .map(|account| (address, account.data))
                .ok_or_else(|| MakerError::VaultState {
                    vault,
                    reason: format!("pricing account {address} does not exist"),
                })
        })
        .collect::<Result<Vec<_>, MakerError>>()?;
    let Some(((_, global_config), reserves)) = accounts.split_first() else {
        return Err(MakerError::VaultState {
            vault,
            reason: "vault global config not returned".to_string(),
        });
    };
    let reserves: Vec<(Address, &[u8])> = reserves
        .iter()
        .map(|(address, data)| (*address, data.as_slice()))
        .collect();
    VaultState::from_accounts(&vault_data, global_config, &reserves).map_err(parse)
}

/// Reads `pair.vault` with [`read_vault`], the path every quote prices
/// through, and checks the pair's addresses against it with `check_pair`.
///
/// Errors with `MakerError::VaultMissing` when the vault account does not
/// exist (or the rpc does not show it yet), `MakerError::VaultState` when a
/// pricing account is missing or does not parse, and
/// `MakerError::Config(ConfigError::VaultMismatch)` when the pair's
/// `token_mint`, `shares_mint`, `token_vault` or `authority` is not the
/// vault's.
pub async fn check_vault(rpc: &dyn AsyncRpc, pair: &Pair) -> Result<(), MakerError> {
    let state = read_vault(rpc, pair.vault).await?;
    Ok(check_pair(pair, &state)?)
}

use std::env;

use anyhow::{anyhow, Result};
use solana_account::Account;
use solana_address::Address;
use zolana_client::{Rpc, SolanaRpc};
use zolana_interface::pda::spl_token_program_id;

use k_lend_rfq_sdk::{
    kvault::{self, TOKEN_ACCOUNT_AMOUNT_OFFSET},
    pair::VaultState,
};

/// Environment variable naming the mainnet RPC the snapshot tests read the
/// vault from; the tests skip when it is unset or empty.
pub const MAINNET_RPC_URL_VAR: &str = "KAMINO_MAINNET_RPC_URL";

/// Kamino "USDC Prime" kVault on mainnet: a USDC vault invested in three
/// klend reserves, without management or performance fees (checked on
/// 2026-10-10 with `chain::read_vault`).
pub const MAINNET_USDC_VAULT: Address =
    Address::from_str_const("9E69U4GzWhryRaPe8DYpco6Z9vTZY6gg8w6W2QsBACEj");

/// The most accounts one `getMultipleAccounts` request may name (the
/// Solana RPC `MAX_MULTIPLE_ACCOUNTS`).
const MAX_ACCOUNTS_PER_REQUEST: usize = 100;

/// Byte length of an SPL Token account.
const TOKEN_ACCOUNT_SIZE: usize = 165;
/// Offsets in the SPL Token account layout: mint (32 bytes), owner (32
/// bytes), amount (u64, at `TOKEN_ACCOUNT_AMOUNT_OFFSET`), delegate
/// (`COption<Pubkey>`, 36 bytes), then the one-byte account state.
const TOKEN_ACCOUNT_OWNER_OFFSET: usize = 32;
const TOKEN_ACCOUNT_STATE_OFFSET: usize = 108;
/// `AccountState::Initialized`.
const TOKEN_ACCOUNT_INITIALIZED: u8 = 1;
/// Rent-exempt minimum of a 165-byte account at the default rent.
const TOKEN_ACCOUNT_LAMPORTS: u64 = 2_039_280;

/// The value of [`MAINNET_RPC_URL_VAR`], or `None` when it is unset or
/// empty.
pub fn mainnet_rpc_url() -> Option<String> {
    env::var(MAINNET_RPC_URL_VAR)
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// Every account a deposit into or withdrawal from `vault` touches, read
/// from the cluster at `rpc_url`, as `(address, account)` pairs to write
/// into a localnet:
///
/// 1. the vault (one `get_account`);
/// 2. its pricing accounts, the kVault `GlobalConfig` and the allocated
///    reserves ([`VaultState::pricing_accounts`]);
/// 3. the vault's token vault, share mint, token mint and base vault
///    authority, and per reserve its lending market, lending market
///    authority, liquidity supply vault, collateral mint and the vault's
///    cToken vault for it.
///
/// Steps 2 and 3 use `get_multiple_accounts` in chunks of at most 100.
/// Accounts of steps 1 and 2 must exist; of step 3 the base vault authority
/// and the lending market authorities are PDAs that hold no data and are
/// left out when unfunded, the others must exist.
pub fn snapshot_vault(rpc_url: &str, vault: &Address) -> Result<Vec<(Address, Account)>> {
    let rpc = SolanaRpc::new(rpc_url);
    let vault_account = rpc
        .get_account(*vault)?
        .ok_or_else(|| anyhow!("vault {vault} missing on {rpc_url}"))?;
    let view = kvault::vault_state(&vault_account.data)?;
    let pricing = VaultState::pricing_accounts(&vault_account.data)?;
    let pricing = required(&rpc, &pricing)?;
    let reserves = pricing
        .iter()
        .skip(1)
        .map(|(address, account)| Ok((*address, kvault::reserve(&account.data)?)))
        .collect::<Result<Vec<_>>>()?;

    let mut needed = vec![view.token_vault, view.shares_mint, view.token_mint];
    let mut optional = vec![view.base_vault_authority];
    for (address, reserve) in &reserves {
        needed.extend([
            reserve.lending_market,
            reserve.liquidity_supply_vault,
            reserve.collateral_mint,
            kvault::ctoken_vault(vault, address),
        ]);
        optional.push(kvault::lending_market_authority(&reserve.lending_market));
    }
    needed.sort();
    needed.dedup();
    optional.sort();
    optional.dedup();
    let needed_count = needed.len();
    let fetched = fetch(&rpc, &[needed, optional].concat())?;

    let mut accounts = vec![(*vault, vault_account)];
    accounts.extend(pricing);
    for (index, (address, account)) in fetched.into_iter().enumerate() {
        match account {
            Some(account) => accounts.push((address, account)),
            None if index < needed_count => {
                return Err(anyhow!("account {address} of vault {vault} missing"))
            }
            None => {}
        }
    }
    Ok(accounts)
}

/// An initialized SPL Token account of `mint` owned by `owner` holding
/// `amount`, to write with `surfnet_setAccount`. The mint's supply is not
/// adjusted; the token program's `transfer` does not read it. Errors if a
/// field lies outside `TOKEN_ACCOUNT_SIZE` bytes.
pub fn token_account(mint: &Address, owner: &Address, amount: u64) -> Result<Account> {
    let mut data = vec![0u8; TOKEN_ACCOUNT_SIZE];
    for (offset, bytes) in [
        (0, mint.as_ref()),
        (TOKEN_ACCOUNT_OWNER_OFFSET, owner.as_ref()),
        (TOKEN_ACCOUNT_AMOUNT_OFFSET, amount.to_le_bytes().as_slice()),
        (
            TOKEN_ACCOUNT_STATE_OFFSET,
            [TOKEN_ACCOUNT_INITIALIZED].as_slice(),
        ),
    ] {
        let range = offset..offset + bytes.len();
        data.get_mut(range.clone())
            .ok_or_else(|| {
                anyhow!("token account field {range:?} outside {TOKEN_ACCOUNT_SIZE} bytes")
            })?
            .copy_from_slice(bytes);
    }
    Ok(Account {
        lamports: TOKEN_ACCOUNT_LAMPORTS,
        data,
        owner: spl_token_program_id(),
        executable: false,
        rent_epoch: 0,
    })
}

/// `addresses` with their accounts; errors when one is missing.
fn required(rpc: &SolanaRpc, addresses: &[Address]) -> Result<Vec<(Address, Account)>> {
    fetch(rpc, addresses)?
        .into_iter()
        .map(|(address, account)| {
            account
                .map(|account| (address, account))
                .ok_or_else(|| anyhow!("account {address} missing"))
        })
        .collect()
}

/// `addresses` with their accounts (`None` when missing), in chunks of
/// [`MAX_ACCOUNTS_PER_REQUEST`].
fn fetch(rpc: &SolanaRpc, addresses: &[Address]) -> Result<Vec<(Address, Option<Account>)>> {
    let mut accounts = Vec::with_capacity(addresses.len());
    for chunk in addresses.chunks(MAX_ACCOUNTS_PER_REQUEST) {
        let fetched = rpc.get_multiple_accounts(chunk.to_vec())?;
        if fetched.len() != chunk.len() {
            return Err(anyhow!(
                "get_multiple_accounts returned {} accounts for {}",
                fetched.len(),
                chunk.len()
            ));
        }
        accounts.extend(chunk.iter().copied().zip(fetched));
    }
    Ok(accounts)
}

#[cfg(test)]
mod tests {
    //! Tested invariants:
    //! 1. `token_account` writes an SPL Token account: mint at 0, owner at 32,
    //!    little-endian amount at 64, state `Initialized` at 108, owned by the
    //!    token program.

    use super::*;

    /// Invariant 1: the account bytes match a layout written by hand.
    #[test]
    fn token_account_lays_out_mint_owner_amount_and_state() {
        let mint = Address::new_from_array([1; 32]);
        let owner = Address::new_from_array([2; 32]);
        let account = token_account(&mint, &owner, 0x0102_0304_0506_0708).expect("the fields fit");
        let mut want = vec![0u8; TOKEN_ACCOUNT_SIZE];
        want.get_mut(..32).expect("mint").fill(1);
        want.get_mut(32..64).expect("owner").fill(2);
        want.get_mut(64..72)
            .expect("amount")
            .copy_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
        *want.get_mut(108).expect("state") = 1;
        assert_eq!(account.data, want, "token account bytes");
        assert_eq!(account.owner, spl_token_program_id(), "token account owner");
    }
}

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_message::VersionedMessage;
use solana_rpc_client_api::config::{
    RpcSimulateTransactionAccountsConfig, RpcSimulateTransactionConfig, UiAccountEncoding,
};
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{
    compile_message, sign_transaction, ComputeBudgetConfig, Rpc, SolanaRpc, ZolanaClient,
};
use zolana_interface::pda;

use k_lend_market_maker::{MAX_COMPUTE_UNITS, SWAP_COMPUTE_BUDGET};
use k_lend_rfq_sdk::{
    kvault::token_account_amount,
    pair::{Pair, VaultState},
};

use crate::kvault;

pub fn blocking<R>(work: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(work),
        Err(_) => work(),
    }
}

/// Reads `pair.vault`, then its `GlobalConfig` and every allocated reserve in
/// one `get_multiple_accounts`, and prices it with
/// [`VaultState::from_accounts`]: the market maker's `read_vault` over the sync
/// rpc.
///
/// Two round trips: the reserves are read at the same or a later slot than
/// the vault, against the allocations of the first read. Errors when any of
/// the accounts is missing or fails to decode.
pub fn read_vault(rpc: &SolanaRpc, pair: &Pair) -> Result<VaultState> {
    blocking(|| {
        let vault = pair.vault;
        let vault_data = rpc
            .get_account(vault)?
            .ok_or_else(|| anyhow!("account {vault} missing"))?
            .data;
        let addresses = VaultState::pricing_accounts(&vault_data)?;
        let fetched = rpc.get_multiple_accounts(addresses.clone())?;
        let mut accounts = addresses
            .into_iter()
            .zip(fetched)
            .map(|(address, account)| {
                account
                    .map(|account| (address, account.data))
                    .ok_or_else(|| anyhow!("account {address} missing"))
            });
        let (_, global_config) = accounts
            .next()
            .ok_or_else(|| anyhow!("vault global config not returned"))??;
        let reserves = accounts.collect::<Result<Vec<_>>>()?;
        let reserves: Vec<(Address, &[u8])> = reserves
            .iter()
            .map(|(address, reserve_data)| (*address, reserve_data.as_slice()))
            .collect();
        VaultState::from_accounts(&vault_data, &global_config, &reserves)
    })
}

/// How often [`wait_for_account`] polls.
const ACCOUNT_POLL: Duration = Duration::from_millis(200);

/// Polls `get_account` until `address` exists; errors once `timeout` passes
/// without it, or on an rpc error.
pub fn wait_for_account(rpc: &SolanaRpc, address: &Address, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while rpc.get_account(*address)?.is_none() {
        if Instant::now() >= deadline {
            return Err(anyhow!("account {address} not readable after {timeout:?}"));
        }
        std::thread::sleep(ACCOUNT_POLL);
    }
    Ok(())
}

/// Waits until the indexer has indexed `signature`; the error names it as
/// `index {what} {signature}`.
pub fn confirm_indexed(
    client: &ZolanaClient<SolanaRpc>,
    signature: Signature,
    what: &str,
) -> Result<()> {
    blocking(|| client.confirm_private_transaction_sync(signature))
        .map_err(|error| anyhow!("index {what} {signature}: {error:?}"))
}

/// Compiles `instructions` paid by `payer` on the latest blockhash with the
/// market maker's `SWAP_COMPUTE_BUDGET`.
pub fn compile_swap(
    rpc: &SolanaRpc,
    payer: &Address,
    instructions: &[Instruction],
) -> Result<VersionedMessage> {
    let (blockhash, _) = blocking(|| rpc.get_latest_blockhash())?;
    Ok(compile_message(
        payer,
        instructions,
        blockhash,
        SWAP_COMPUTE_BUDGET,
    )?)
}

pub fn send(
    rpc: &SolanaRpc,
    instructions: &[Instruction],
    payer: &dyn Signer,
    signers: &[&dyn Signer],
) -> Result<Signature> {
    Ok(rpc.create_and_send_transaction(
        instructions,
        payer.pubkey(),
        signers,
        ComputeBudgetConfig::new(MAX_COMPUTE_UNITS),
    )?)
}

/// What a simulated transaction did: the SPL token amount of the watched
/// account afterwards, the compute units it consumed and its logs.
#[derive(Debug)]
pub struct Simulated {
    pub token_amount: u64,
    pub units_consumed: u64,
    pub logs: Vec<String>,
}

/// Simulates `instructions` paid and signed by `payer` with a budget of
/// `compute_units`, on the latest blockhash and without signature
/// verification, and reads `account`'s SPL token amount afterwards. A
/// transaction error is returned with the full program logs.
pub fn simulate_token_balance(
    rpc: &SolanaRpc,
    instructions: &[Instruction],
    payer: &dyn Signer,
    compute_units: u32,
    account: &Address,
) -> Result<Simulated> {
    let message = compile_message(
        &payer.pubkey(),
        instructions,
        solana_hash::Hash::default(),
        ComputeBudgetConfig::new(compute_units),
    )?;
    let transaction = sign_transaction(message, &[payer])?;
    let simulated = rpc
        .client()
        .simulate_transaction_with_config(
            &transaction,
            RpcSimulateTransactionConfig {
                sig_verify: false,
                replace_recent_blockhash: true,
                accounts: Some(RpcSimulateTransactionAccountsConfig {
                    encoding: Some(UiAccountEncoding::Base64),
                    addresses: vec![account.to_string()],
                }),
                ..RpcSimulateTransactionConfig::default()
            },
        )?
        .value;
    let logs = simulated.logs.unwrap_or_default();
    if let Some(error) = simulated.err {
        return Err(anyhow!(
            "simulation failed: {error:?}; logs:\n{}",
            logs.join("\n")
        ));
    }
    let account_data = simulated
        .accounts
        .and_then(|accounts| accounts.into_iter().next())
        .flatten()
        .ok_or_else(|| anyhow!("simulation returned no account {account}"))?
        .data
        .decode()
        .ok_or_else(|| anyhow!("account {account} data does not decode"))?;
    Ok(Simulated {
        token_amount: token_account_amount(&account_data)
            .ok_or_else(|| anyhow!("token account {account} too short"))?,
        units_consumed: simulated
            .units_consumed
            .ok_or_else(|| anyhow!("simulation reports no compute units"))?,
        logs,
    })
}

pub fn compute_units(rpc: &SolanaRpc, signature: &Signature) -> Result<u64> {
    let confirmed = rpc.fetch_confirmed_transaction(signature)?;
    let meta = confirmed
        .transaction
        .meta
        .ok_or_else(|| anyhow!("transaction {signature} has no metadata"))?;
    Option::<u64>::from(meta.compute_units_consumed)
        .ok_or_else(|| anyhow!("transaction {signature} reports no compute units"))
}

/// The balances of `owner`'s public collateral and share accounts of `pair`,
/// where a market maker's kVault tail leaves what it does not deposit or
/// shield.
pub fn public_balances(rpc: &SolanaRpc, owner: &Address, pair: &Pair) -> Result<[u64; 2]> {
    let balance =
        |mint: &Address| kvault::token_balance(rpc, &pda::associated_token_address(owner, mint));
    blocking(|| Ok([balance(&pair.token_mint)?, balance(&pair.shares_mint)?]))
}

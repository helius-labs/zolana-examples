//! The market maker's configuration: what it is started with, and the
//! `Settings` it runs on. Every setting is validated where it enters, at
//! start (`Settings::new`) or in an update, so the running settings always
//! satisfy the checks of `ConfigError`.

mod update;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use solana_address::Address;
use solana_signer::Signer;
use zolana_client::ProofAuthority;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::ShieldedKeys;

use thiserror::Error;

use k_lend_rfq_sdk::{
    kvault,
    pair::{Pair, VaultState},
    swap::FULL_BPS,
};

use crate::{error::MakerError, inventory::balance::profile::InventoryProfile};

pub use update::{ConfigUpdate, RangeChange, RangeUpdate};

/// Default smallest UTXO the inventory profile creates, in asset atoms: one
/// whole unit of a 6-decimal mint. A tuning default, not a protocol limit.
const MIN_UTXO_VALUE: u64 = 1_000_000;

/// A setting the market maker refuses, at startup (`Settings::new`) or in a
/// [`ConfigUpdate`].
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("target range min {min} exceeds max {max}")]
    InvertedRange { min: u64, max: u64 },

    #[error("fee_bps {fee_bps} exceeds 10000")]
    FeeAboveFull { fee_bps: u64 },

    #[error("order_ttl must be greater than zero")]
    ZeroOrderTtl,

    #[error("pair of vault {vault} is named twice")]
    DuplicatePair { vault: Address },

    #[error("no pair of vault {vault} is configured")]
    UnknownPair { vault: Address },

    #[error("mint {mint} belongs to no configured pair")]
    UnknownAsset { mint: Address },

    #[error("token config of mint {mint} is given twice")]
    DuplicateToken { mint: Address },

    #[error("pair of vault {vault} is already configured")]
    PairExists { vault: Address },

    /// A period of `ConcurrencyConfig` is zero: `name` is
    /// `status_interval`, `sync_interval` or `utxo_upkeep_delay`.
    #[error("{name} must be greater than zero")]
    ZeroInterval { name: &'static str },

    #[error("provers must be at least one")]
    ZeroProvers,

    /// A pair address does not belong to its vault: `field` is
    /// `token_mint` (differs from the vault account's mint), or
    /// `shares_mint`, `token_vault` or `authority` (differs from the PDA
    /// of that name derived from the vault address).
    #[error("pair of vault {vault} has a {field} that is not the vault's")]
    VaultMismatch { vault: Address, field: &'static str },
}

/// Everything `MarketMaker::start` needs.
pub struct MarketMakerConfig {
    pub connection: ConnectionConfig,
    pub identity: IdentityConfig,
    pub pairs: Vec<Pair>,
    /// Target range and UTXO profile per mint. Every mint must belong to one
    /// of `pairs`; a mint shared by several pairs has one entry that applies
    /// to all of them. A mint without an entry has no range and uses
    /// `concurrency.profile`.
    pub tokens: Vec<(Address, TokenConfig)>,
    pub concurrency: ConcurrencyConfig,
    pub quotes: QuoteConfig,
}

/// The services the maker talks to.
#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    /// Solana json-rpc.
    pub rpc_url: String,
    /// The zolana indexer.
    pub photon_url: String,
    /// The prover server; `None` uses the client's default address.
    pub prover_url: Option<String>,
    /// The state tree the maker's UTXOs live in and its outputs go to.
    pub tree: Address,
    /// The zolana id of `tree`.
    pub tree_id: u16,
}

/// The market maker's wallet, split into the three roles it acts in.
///
/// - `keys` opens the outputs addressed to the maker (sync and fill checks),
///   derives its nullifiers and per-transaction viewing keys, and names its
///   shielded address.
/// - `authority` puts the nullifier secret into a proof witness, the one place
///   the secret itself is consumed.
/// - `signer` signs every Solana transaction as fee payer, including the
///   co-signature on a swap; its pubkey is the maker's fee payer address.
///
/// Each role is an operation, not key material: none of the three needs the
/// secret in this process, so a TEE or a remote key holder can implement them.
/// [`IdentityConfig::from_keypair`] is the in-process case where one
/// `ShieldedKeypair` answers all three.
pub struct IdentityConfig {
    pub keys: Arc<dyn ShieldedKeys + Send + Sync>,
    pub authority: Arc<dyn ProofAuthority>,
    pub signer: Arc<dyn Signer + Send + Sync>,
}

impl IdentityConfig {
    /// All three roles answered by one in-process keypair.
    pub fn from_keypair(keypair: Arc<ShieldedKeypair>) -> Self {
        Self {
            keys: keypair.clone(),
            authority: keypair.clone(),
            signer: keypair,
        }
    }
}

/// The inventory settings of one mint, shared by every pair that trades it.
#[derive(Clone, Debug, Default)]
pub struct TokenConfig {
    /// The balance the maker keeps the mint in: quotes that would leave it
    /// are refused and automatic rebalances steer back into it. `None`
    /// means unbounded.
    pub range: Option<TargetRange>,
    /// The UTXO profile; `None` uses `ConcurrencyConfig::profile`.
    pub profile: Option<InventoryProfile>,
}

/// An inclusive balance range `min..=max`. Invariant: `min <= max`, enforced
/// by [`TargetRange::new`], the only constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRange {
    min: u64,
    max: u64,
}

impl TargetRange {
    /// Rejects `min > max` with [`ConfigError::InvertedRange`].
    pub const fn new(min: u64, max: u64) -> Result<Self, ConfigError> {
        if min > max {
            return Err(ConfigError::InvertedRange { min, max });
        }
        Ok(Self { min, max })
    }

    /// The inclusive lower bound.
    pub const fn min(&self) -> u64 {
        self.min
    }

    /// The inclusive upper bound.
    pub const fn max(&self) -> u64 {
        self.max
    }

    /// Whether `min <= balance <= max`.
    pub fn contains(&self, balance: u64) -> bool {
        (self.min..=self.max).contains(&balance)
    }

    /// The midpoint, rounded down. Cannot underflow: `min <= max` holds for
    /// every `TargetRange`.
    pub fn middle(&self) -> u64 {
        self.min + (self.max - self.min) / 2
    }
}

/// Runtime tuning. The defaults are tuning choices for localnet, not
/// protocol limits.
#[derive(Clone, Debug)]
pub struct ConcurrencyConfig {
    /// The UTXO profile of a mint without its own.
    pub profile: InventoryProfile,
    /// Most UTXOs one shield instruction creates.
    pub max_shield_utxos: usize,
    /// Idle time after which upkeep consolidations run; `None` disables
    /// upkeep. Non-zero; also the base of the automatic rebalance backoff
    /// (`Coordinator::check_ranges`).
    pub utxo_upkeep_delay: Option<Duration>,
    /// Number of proofs generated at the same time, at least one. Each
    /// worker holds one prover request (CPU-bound in the prover process)
    /// for the whole proof plus the blockhash fetch that follows it.
    pub provers: usize,
    /// Period of the coordinator's status poll of sent transactions, also
    /// the backoff before a resend. Non-zero.
    pub status_interval: Duration,
    /// Period of the background indexer sync. Non-zero.
    pub sync_interval: Duration,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            profile: InventoryProfile::equal(1, MIN_UTXO_VALUE),
            max_shield_utxos: 8,
            utxo_upkeep_delay: None,
            provers: 4,
            status_interval: Duration::from_millis(400),
            sync_interval: Duration::from_millis(1_000),
        }
    }
}

/// Quoting terms; both can be changed at runtime by a `ConfigUpdate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuoteConfig {
    /// Taken off the vault price of every quote; at most `FULL_BPS`. It has
    /// to cover the drift between the quoted and the executed vault price
    /// (see `VaultState`).
    pub fee_bps: u64,
    /// An offer expires this long after it was quoted; the maker checks the
    /// deadline when the fill request arrives and the user checks it before
    /// proving.
    pub order_ttl: Duration,
}

impl Default for QuoteConfig {
    fn default() -> Self {
        Self {
            fee_bps: 30,
            order_ttl: Duration::from_secs(60),
        }
    }
}

/// Rejects a `fee_bps` above [`FULL_BPS`] with [`ConfigError::FeeAboveFull`]:
/// the fee would exceed the gross amount and every quote would pay zero.
fn check_fee_bps(fee_bps: u64) -> Result<(), ConfigError> {
    if fee_bps > FULL_BPS {
        return Err(ConfigError::FeeAboveFull { fee_bps });
    }
    Ok(())
}

/// Rejects an `order_ttl` of zero with [`ConfigError::ZeroOrderTtl`]: every
/// offer would be expired the moment it is quoted.
fn check_order_ttl(order_ttl: Duration) -> Result<(), ConfigError> {
    if order_ttl.is_zero() {
        return Err(ConfigError::ZeroOrderTtl);
    }
    Ok(())
}

/// Rejects a `ConcurrencyConfig` the maker cannot run on:
/// - a zero `status_interval` or `sync_interval` with
///   [`ConfigError::ZeroInterval`]: `tokio::time::interval` panics on a zero
///   period, which would stop the coordinator or the sync task;
/// - a zero `utxo_upkeep_delay` with [`ConfigError::ZeroInterval`]: it is
///   also the base of the rebalance backoff, which would not wait at all
///   (`None` disables upkeep and is accepted);
/// - zero `provers` with [`ConfigError::ZeroProvers`]: no proof could run.
fn check_concurrency(concurrency: &ConcurrencyConfig) -> Result<(), ConfigError> {
    for (name, interval) in [
        ("status_interval", Some(concurrency.status_interval)),
        ("sync_interval", Some(concurrency.sync_interval)),
        ("utxo_upkeep_delay", concurrency.utxo_upkeep_delay),
    ] {
        if interval.is_some_and(|interval| interval.is_zero()) {
            return Err(ConfigError::ZeroInterval { name });
        }
    }
    if concurrency.provers == 0 {
        return Err(ConfigError::ZeroProvers);
    }
    Ok(())
}

/// Rejects a vault named by two pairs of `pairs` with
/// [`ConfigError::DuplicatePair`].
fn check_distinct_pairs(pairs: &[Pair]) -> Result<(), ConfigError> {
    let mut vaults = HashSet::new();
    for pair in pairs {
        if !vaults.insert(pair.vault) {
            return Err(ConfigError::DuplicatePair { vault: pair.vault });
        }
    }
    Ok(())
}

/// Rejects a `pair` whose addresses do not belong to its vault with
/// [`ConfigError::VaultMismatch`], checking in order:
/// - `token_mint` against `state.token_mint`, the mint in the vault account;
/// - `shares_mint`, `token_vault` and `authority` against the kVault PDAs of
///   `pair.vault` ([`kvault::shares_mint`], [`kvault::token_vault`],
///   [`kvault::base_vault_authority`]).
///
/// `state` is the vault as `read_vault` prices it, so a vault that passes
/// also has every account the quote path reads.
pub(crate) fn check_pair(pair: &Pair, state: &VaultState) -> Result<(), ConfigError> {
    let vault = pair.vault;
    for (field, configured, expected) in [
        ("token_mint", pair.token_mint, state.token_mint),
        ("shares_mint", pair.shares_mint, kvault::shares_mint(&vault)),
        ("token_vault", pair.token_vault, kvault::token_vault(&vault)),
        (
            "authority",
            pair.authority,
            kvault::base_vault_authority(&vault),
        ),
    ] {
        if configured != expected {
            return Err(ConfigError::VaultMismatch { vault, field });
        }
    }
    Ok(())
}

/// The two mints of `pair`: collateral first, then shares.
const fn pair_assets(pair: &Pair) -> [Address; 2] {
    [pair.token_mint, pair.shares_mint]
}

/// The validated settings the maker runs on.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub quotes: QuoteConfig,
    /// Vaults removed by an update: no quotes or fills (open orders on them
    /// fail with `MakerError::PairNotServed`); queued operations and
    /// in-flight steps finish before `drop_retired` removes the pair.
    pub retiring: HashSet<Address>,
    pub profile: InventoryProfile,
    pub max_shield_utxos: usize,
    pub idle_delay: Option<Duration>,
    pub status_interval: Duration,
    pub pairs: Vec<Pair>,
    /// Range and profile per mint; keys are assets of `pairs`.
    pub tokens: HashMap<Address, TokenConfig>,
}

impl Settings {
    /// Builds the startup settings. Rejects a zero interval or zero provers
    /// in `concurrency` (`check_concurrency`: [`ConfigError::ZeroInterval`],
    /// [`ConfigError::ZeroProvers`]), `quotes.fee_bps > FULL_BPS`
    /// ([`ConfigError::FeeAboveFull`]), a zero `quotes.order_ttl`
    /// ([`ConfigError::ZeroOrderTtl`]), two pairs of the same vault
    /// ([`ConfigError::DuplicatePair`]), a token config for a mint of no pair
    /// ([`ConfigError::UnknownAsset`]) and two token configs for one mint
    /// ([`ConfigError::DuplicateToken`]).
    pub fn new(
        concurrency: &ConcurrencyConfig,
        quotes: QuoteConfig,
        pairs: Vec<Pair>,
        tokens: Vec<(Address, TokenConfig)>,
    ) -> Result<Self, ConfigError> {
        check_concurrency(concurrency)?;
        check_fee_bps(quotes.fee_bps)?;
        check_order_ttl(quotes.order_ttl)?;
        check_distinct_pairs(&pairs)?;
        let mut configs = HashMap::with_capacity(tokens.len());
        for (mint, token) in tokens {
            if !pairs.iter().any(|pair| pair_assets(pair).contains(&mint)) {
                return Err(ConfigError::UnknownAsset { mint });
            }
            if configs.insert(mint, token).is_some() {
                return Err(ConfigError::DuplicateToken { mint });
            }
        }
        Ok(Self {
            quotes,
            retiring: HashSet::new(),
            profile: concurrency.profile.clone(),
            max_shield_utxos: concurrency.max_shield_utxos,
            idle_delay: concurrency.utxo_upkeep_delay,
            status_interval: concurrency.status_interval,
            pairs,
            tokens: configs,
        })
    }

    /// The mints of all configured pairs; a mint shared by two pairs appears
    /// once.
    pub fn assets(&self) -> Vec<Address> {
        let mut assets = Vec::new();
        for asset in self.pairs.iter().flat_map(pair_assets) {
            if !assets.contains(&asset) {
                assets.push(asset);
            }
        }
        assets
    }

    /// The configured pair of `vault`, retiring or not.
    pub fn pair(&self, vault: &Address) -> Option<&Pair> {
        self.pairs.iter().find(|pair| pair.vault == *vault)
    }

    /// Whether the maker quotes and fills on `pair`: it is configured with
    /// the same token mint and PDAs and is not retiring, else
    /// `MakerError::PairNotServed`.
    pub fn serves(&self, pair: &Pair) -> Result<(), MakerError> {
        let served = self
            .pair(&pair.vault)
            .is_some_and(|configured| configured == pair)
            && !self.retiring.contains(&pair.vault);
        if served {
            Ok(())
        } else {
            Err(MakerError::PairNotServed { vault: pair.vault })
        }
    }

    /// The target range of `asset`, the same for every pair that trades it.
    pub fn range(&self, asset: &Address) -> Option<TargetRange> {
        self.tokens.get(asset).and_then(|token| token.range)
    }

    /// The UTXO profile of `asset`, or the global profile if `asset` has
    /// none.
    pub fn profile(&self, asset: &Address) -> &InventoryProfile {
        self.tokens
            .get(asset)
            .and_then(|token| token.profile.as_ref())
            .unwrap_or(&self.profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(concurrency: ConcurrencyConfig) -> Result<Settings, ConfigError> {
        Settings::new(&concurrency, QuoteConfig::default(), Vec::new(), Vec::new())
    }

    /// A zero status or sync interval, a zero upkeep delay and zero provers
    /// are refused at start; `None` upkeep and the defaults are accepted.
    #[test]
    fn concurrency_rejects_zero_periods_and_provers() {
        let defaults = ConcurrencyConfig::default;
        for (label, concurrency, want) in [
            (
                "zero status_interval",
                ConcurrencyConfig {
                    status_interval: Duration::ZERO,
                    ..defaults()
                },
                Some("status_interval"),
            ),
            (
                "zero sync_interval",
                ConcurrencyConfig {
                    sync_interval: Duration::ZERO,
                    ..defaults()
                },
                Some("sync_interval"),
            ),
            (
                "zero utxo_upkeep_delay",
                ConcurrencyConfig {
                    utxo_upkeep_delay: Some(Duration::ZERO),
                    ..defaults()
                },
                Some("utxo_upkeep_delay"),
            ),
        ] {
            let got = settings(concurrency).err();
            assert!(
                matches!(got, Some(ConfigError::ZeroInterval { name }) if Some(name) == want),
                "{label}: got {got:?}, want ZeroInterval {{ name: {want:?} }}"
            );
        }
        let zero_provers = settings(ConcurrencyConfig {
            provers: 0,
            ..defaults()
        })
        .err();
        assert!(
            matches!(zero_provers, Some(ConfigError::ZeroProvers)),
            "zero provers: got {zero_provers:?}, want ZeroProvers"
        );
        for (label, concurrency) in [
            ("defaults", defaults()),
            (
                "upkeep after one millisecond",
                ConcurrencyConfig {
                    utxo_upkeep_delay: Some(Duration::from_millis(1)),
                    ..defaults()
                },
            ),
        ] {
            let got = settings(concurrency).err();
            assert!(got.is_none(), "{label}: got {got:?}, want Ok");
        }
    }

    /// A pair built by `Pair::new` on the vault's mint passes; each address
    /// replaced by another one fails with `VaultMismatch` naming that field.
    #[test]
    fn check_pair_rejects_addresses_not_of_the_vault() {
        let mint = Address::new_from_array([1; 32]);
        let other = Address::new_from_array([9; 32]);
        let state = VaultState {
            token_mint: mint,
            token_program: Address::new_from_array([2; 32]),
            token_available: 0,
            shares_issued: 0,
            pending_fees_sf: 0,
            reserves: k_lend_rfq_sdk::pair::Reserves::EMPTY,
            min_deposit_amount: 0,
            min_withdraw_amount: 0,
            deposit_cap: 0,
            crank_funds: 0,
            withdrawal_penalty_lamports: 0,
            withdrawal_penalty_bps: 0,
            global_withdrawal_penalty_lamports: 0,
            global_withdrawal_penalty_bps: 0,
        };
        let pair = Pair::new(Address::new_from_array([10; 32]), mint);
        let got = check_pair(&pair, &state).err();
        assert!(got.is_none(), "matching pair: got {got:?}, want Ok");
        for (want, broken) in [
            (
                "token_mint",
                Pair {
                    token_mint: other,
                    ..pair
                },
            ),
            (
                "shares_mint",
                Pair {
                    shares_mint: other,
                    ..pair
                },
            ),
            (
                "token_vault",
                Pair {
                    token_vault: other,
                    ..pair
                },
            ),
            (
                "authority",
                Pair {
                    authority: other,
                    ..pair
                },
            ),
        ] {
            let got = check_pair(&broken, &state).err();
            assert!(
                matches!(
                    got,
                    Some(ConfigError::VaultMismatch { vault, field })
                        if vault == pair.vault && field == want
                ),
                "{want}: got {got:?}, want VaultMismatch {{ field: {want:?} }}"
            );
        }
    }
}

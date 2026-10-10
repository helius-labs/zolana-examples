//! Runtime configuration updates. An update is validated whole against the
//! current settings and applied atomically on the coordinator; removed pairs
//! retire and are dropped only once nothing in flight uses them.

use std::{sync::PoisonError, time::Duration};

use solana_address::Address;
use tokio::sync::oneshot;

use k_lend_rfq_sdk::pair::Pair;

use super::{
    check_distinct_pairs, check_fee_bps, check_order_ttl, pair_assets, ConfigError, Settings,
    TargetRange,
};
use crate::{
    api::Inner,
    error::MarketMakerError,
    inventory::balance::profile::InventoryProfile,
    inventory::balance::sync::registered_asset,
    transactions::{
        coordinator::{Coordinator, Event, Operation},
        kvault::check_vault,
    },
};

/// A change to the running settings. `None` and empty lists leave the
/// setting as it is. Added pairs are checked against their vaults on chain
/// before the update is applied (see `add_pairs`).
#[derive(Clone, Debug, Default)]
pub struct ConfigUpdate {
    /// New quote fee; applies to quotes issued after the update.
    pub fee_bps: Option<u64>,
    /// New order TTL; orders already open keep their expiry.
    pub order_ttl: Option<Duration>,
    pub ranges: Vec<RangeUpdate>,
    /// New UTXO profile per mint.
    pub utxo_profiles: Vec<(Address, InventoryProfile)>,
    /// Pairs to start serving. Before anything changes, each one's vault is
    /// read and checked like at start (`check_vault`): a vault that does not
    /// exist fails with `MarketMakerError::VaultMissing`, a missing or
    /// unparsable pricing account with `MarketMakerError::VaultState`, and a
    /// `token_mint` that is not the vault's mint or a `shares_mint`,
    /// `token_vault` or `authority` that is not the vault's PDA with
    /// `MarketMakerError::Config(ConfigError::VaultMismatch)`. Their mints must
    /// then be registered in the pool (`MarketMakerError::AssetNotRegistered`).
    pub add_pairs: Vec<Pair>,
    /// Vaults of pairs to stop serving; they retire first.
    pub remove_pairs: Vec<Address>,
}

/// Changes the target range of `asset`, a mint of a pair that is either
/// configured already or added by the same update. The range is per asset,
/// so it applies to every pair that trades `asset`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RangeUpdate {
    pub asset: Address,
    pub range: RangeChange,
}

/// What a [`RangeUpdate`] does to one asset's target range. `Keep` is the
/// default, the same way `fee_bps: None` keeps the fee.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeChange {
    /// Leave the range as it is.
    #[default]
    Keep,
    /// Remove the range: the asset becomes unbounded.
    Clear,
    /// Replace the range.
    Set(TargetRange),
}

impl Settings {
    /// Rejects an update that would leave invalid settings, before any part
    /// of it is applied:
    /// - `fee_bps` above `FULL_BPS`: [`ConfigError::FeeAboveFull`];
    /// - a zero `order_ttl`: [`ConfigError::ZeroOrderTtl`];
    /// - an added vault that is configured (or still retiring):
    ///   [`ConfigError::PairExists`]; added twice:
    ///   [`ConfigError::DuplicatePair`];
    /// - a `remove_pairs` vault that is neither configured nor added by this
    ///   update: [`ConfigError::UnknownPair`];
    /// - a `ranges` asset or profile mint that belongs to no configured or
    ///   added pair: [`ConfigError::UnknownAsset`].
    fn check(&self, update: &ConfigUpdate) -> Result<(), ConfigError> {
        if let Some(fee_bps) = update.fee_bps {
            check_fee_bps(fee_bps)?;
        }
        if let Some(order_ttl) = update.order_ttl {
            check_order_ttl(order_ttl)?;
        }
        for added in &update.add_pairs {
            if self.pair(&added.vault).is_some() {
                return Err(ConfigError::PairExists { vault: added.vault });
            }
        }
        check_distinct_pairs(&update.add_pairs)?;
        for vault in &update.remove_pairs {
            let added = update.add_pairs.iter().any(|pair| pair.vault == *vault);
            if self.pair(vault).is_none() && !added {
                return Err(ConfigError::UnknownPair { vault: *vault });
            }
        }
        let assets = self.assets();
        let known = |mint: &Address| {
            let added = update
                .add_pairs
                .iter()
                .any(|pair| pair_assets(pair).contains(mint));
            if assets.contains(mint) || added {
                Ok(())
            } else {
                Err(ConfigError::UnknownAsset { mint: *mint })
            }
        };
        for range in &update.ranges {
            known(&range.asset)?;
        }
        for (mint, _) in &update.utxo_profiles {
            known(mint)?;
        }
        Ok(())
    }

    /// Applies `update` after [`Settings::check`] accepts it; a rejected
    /// update changes nothing.
    pub fn apply(&mut self, update: ConfigUpdate) -> Result<(), ConfigError> {
        self.check(&update)?;
        let ConfigUpdate {
            fee_bps,
            order_ttl,
            ranges,
            utxo_profiles,
            add_pairs,
            remove_pairs,
        } = update;
        if let Some(fee_bps) = fee_bps {
            self.quotes.fee_bps = fee_bps;
        }
        if let Some(order_ttl) = order_ttl {
            self.quotes.order_ttl = order_ttl;
        }
        self.pairs.extend(add_pairs);
        for change in ranges {
            // `Keep` still adds the asset's default entry.
            let range = &mut self.tokens.entry(change.asset).or_default().range;
            match change.range {
                RangeChange::Keep => {}
                RangeChange::Clear => *range = None,
                RangeChange::Set(new) => *range = Some(new),
            }
        }
        for (mint, profile) in utxo_profiles {
            self.tokens.entry(mint).or_default().profile = Some(profile);
        }
        self.retiring.extend(remove_pairs);
        Ok(())
    }
}

impl Inner {
    /// Applies `update` on the coordinator and, when it adds pairs,
    /// registers their mints and rescans the account so UTXOs of the new
    /// assets are found.
    ///
    /// Each added pair's vault is checked first (`check_vault`, the same check
    /// as `MarketMaker::start`), so a missing vault fails with
    /// `MarketMakerError::VaultMissing` and a pair whose addresses are not the
    /// vault's with `MarketMakerError::Config(ConfigError::VaultMismatch)`
    /// before anything changes. The added mints are resolved next, so an
    /// unregistered one fails with `MarketMakerError::AssetNotRegistered`,
    /// still before anything changes. They are then inserted into the asset
    /// registry before the update reaches the coordinator, so a fill on a new
    /// pair, possible as soon as the coordinator applied it, can always decrypt
    /// the user's outputs; a registry insert error therefore fails the call
    /// before the update is applied. A rejected update fails with
    /// `MarketMakerError::Config` (see `Settings::check`) and changes no
    /// settings.
    ///
    /// Registry entries added for an update that is later rejected (or that
    /// the coordinator never applies) are not rolled back: an entry only maps
    /// a mint to its on-chain asset id, and without a pair using the mint no
    /// quote or fill touches it, so on its own it is harmless.
    pub async fn update_config(&self, update: ConfigUpdate) -> Result<(), MarketMakerError> {
        for pair in &update.add_pairs {
            check_vault(self.services.rpc.as_ref(), pair).await?;
        }
        let mut added = Vec::new();
        for pair in &update.add_pairs {
            for mint in pair_assets(pair) {
                added.push((
                    registered_asset(self.services.rpc.as_ref(), mint).await?,
                    mint,
                ));
            }
        }
        {
            let mut registry = self
                .registry
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            for (asset_id, mint) in &added {
                if registry.asset_id(mint).is_err() {
                    registry.insert(*asset_id, *mint)?;
                }
            }
        }
        let (reply, applied) = oneshot::channel();
        self.runtime
            .events
            .send(Event::UpdateConfig { update, reply })
            .map_err(|_| MarketMakerError::ShuttingDown)?;
        applied
            .await
            .map_err(|_| MarketMakerError::CoordinatorStopped)??;
        if added.is_empty() {
            return Ok(());
        }
        self.account.lock().await.rescan();
        self.sync().await
    }
}

impl Coordinator {
    /// Applies `update` to the coordinator's settings, publishes them to the
    /// api on success, replies, and reschedules since ranges or pairs may
    /// have changed.
    pub async fn on_update_config(
        &mut self,
        update: ConfigUpdate,
        reply: oneshot::Sender<Result<(), MarketMakerError>>,
    ) {
        let applied = self.config.apply(update).map_err(MarketMakerError::from);
        if applied.is_ok() {
            self.publish_config();
        }
        let _ = reply.send(applied);
        self.try_schedule().await;
    }

    /// Removes every retiring pair with no operation in flight or queued, and
    /// the token configs of mints no remaining pair trades.
    pub fn drop_retired(&mut self) {
        let retired: Vec<Address> = self
            .config
            .retiring
            .iter()
            .copied()
            .filter(|vault| !self.pair_busy(vault))
            .collect();
        if retired.is_empty() {
            return;
        }
        self.config
            .pairs
            .retain(|pair| !retired.contains(&pair.vault));
        self.config
            .retiring
            .retain(|vault| !retired.contains(vault));
        let assets = self.config.assets();
        self.config.tokens.retain(|asset, _| assets.contains(asset));
        self.publish_config();
    }

    /// Whether a step in flight, or an operation queued or scheduled, uses
    /// `vault`'s pair or one of its assets.
    fn pair_busy(&self, vault: &Address) -> bool {
        let Some(pair) = self.config.pair(vault) else {
            return false;
        };
        let assets = pair_assets(pair);
        let stepping = self
            .steps
            .in_flight()
            .any(|step| step.asset.is_some_and(|asset| assets.contains(&asset)));
        let queued = self
            .queue
            .iter()
            .chain(self.scheduled.values())
            .any(|queued| match &queued.operation {
                Operation::Rebalance(order) => order.pair.vault == *vault,
                operation => assets.contains(&operation.asset()),
            });
        stepping || queued
    }

    fn publish_config(&self) {
        *self
            .shared_config
            .write()
            .unwrap_or_else(PoisonError::into_inner) = self.config.clone();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use solana_address::Address;

    use k_lend_rfq_sdk::{pair::Pair, swap::FULL_BPS};

    use super::{ConfigUpdate, RangeChange, RangeUpdate};
    use crate::config::{
        ConcurrencyConfig, ConfigError, QuoteConfig, Settings, TargetRange, TokenConfig,
    };

    const COLLATERAL_MINT: Address = Address::new_from_array([1; 32]);

    /// A pair of vault `[seed; 32]` on `COLLATERAL_MINT`.
    fn pair(seed: u8) -> Pair {
        Pair::new(Address::new_from_array([seed; 32]), COLLATERAL_MINT)
    }

    /// Settings with default concurrency and no token configs.
    fn settings(quotes: QuoteConfig, pairs: Vec<Pair>) -> Result<Settings, ConfigError> {
        Settings::new(&ConcurrencyConfig::default(), quotes, pairs, Vec::new())
    }

    /// Default settings serving `pair(10)`.
    fn served() -> Settings {
        settings(QuoteConfig::default(), vec![pair(10)]).expect("valid settings")
    }

    /// A token config with `range` and the default profile.
    fn ranged(range: TargetRange) -> TokenConfig {
        TokenConfig {
            range: Some(range),
            ..TokenConfig::default()
        }
    }

    /// Asserts that `outcome` failed with a `ConfigError` for which `is_want`
    /// holds; `want` names the expected variant.
    #[track_caller]
    fn assert_config_error<T>(
        outcome: Result<T, ConfigError>,
        want: &str,
        is_want: impl FnOnce(&ConfigError) -> bool,
    ) {
        match outcome {
            Ok(_) => panic!("got Ok, want {want}"),
            Err(error) => assert!(is_want(&error), "got {error:?}, want {want}"),
        }
    }

    /// `TargetRange::new` rejects `min > max` with `InvertedRange` and
    /// accepts `min == max`.
    #[test]
    fn target_range_rejects_inverted_bounds() {
        assert_config_error(TargetRange::new(5, 4), "InvertedRange", |error| {
            matches!(error, ConfigError::InvertedRange { min: 5, max: 4 })
        });
        let point = TargetRange::new(4, 4).expect("min == max");
        assert_eq!(point.middle(), 4, "middle of [4, 4]");
    }

    /// A fee above `FULL_BPS` fails with `FeeAboveFull` at start and as an
    /// update, which leaves the quote config unchanged; `FULL_BPS` itself is
    /// accepted.
    #[test]
    fn settings_reject_fee_above_full_bps() {
        let fee_bps = FULL_BPS + 1;
        assert_config_error(
            settings(
                QuoteConfig {
                    fee_bps,
                    ..QuoteConfig::default()
                },
                vec![pair(10)],
            ),
            "FeeAboveFull",
            |error| matches!(error, ConfigError::FeeAboveFull { fee_bps: rejected } if *rejected == fee_bps),
        );
        let mut settings = served();
        assert_config_error(
            settings.apply(ConfigUpdate {
                fee_bps: Some(fee_bps),
                ..ConfigUpdate::default()
            }),
            "FeeAboveFull",
            |error| matches!(error, ConfigError::FeeAboveFull { fee_bps: rejected } if *rejected == fee_bps),
        );
        assert_eq!(settings.quotes, QuoteConfig::default());
        settings
            .apply(ConfigUpdate {
                fee_bps: Some(FULL_BPS),
                ..ConfigUpdate::default()
            })
            .expect("a fee of FULL_BPS is accepted");
        assert_eq!(settings.quotes.fee_bps, FULL_BPS);
    }

    /// A zero `order_ttl` fails with `ZeroOrderTtl` at start and as an
    /// update, which leaves the quote config unchanged.
    #[test]
    fn settings_reject_zero_order_ttl() {
        assert_config_error(
            settings(
                QuoteConfig {
                    order_ttl: Duration::ZERO,
                    ..QuoteConfig::default()
                },
                vec![pair(10)],
            ),
            "ZeroOrderTtl",
            |error| matches!(error, ConfigError::ZeroOrderTtl),
        );
        let mut settings = served();
        assert_config_error(
            settings.apply(ConfigUpdate {
                order_ttl: Some(Duration::ZERO),
                ..ConfigUpdate::default()
            }),
            "ZeroOrderTtl",
            |error| matches!(error, ConfigError::ZeroOrderTtl),
        );
        assert_eq!(settings.quotes, QuoteConfig::default());
    }

    /// A pair added twice in one update or at start fails with
    /// `DuplicatePair`, and a pair already served fails with `PairExists`.
    #[test]
    fn settings_reject_duplicate_or_existing_pair() {
        let added = pair(11);
        let mut settings = served();
        assert_config_error(
            settings.apply(ConfigUpdate {
                add_pairs: vec![added, added],
                ..ConfigUpdate::default()
            }),
            "DuplicatePair",
            |error| matches!(error, ConfigError::DuplicatePair { vault: rejected } if *rejected == added.vault),
        );
        assert!(settings.pair(&added.vault).is_none());
        let existing = pair(10);
        assert_config_error(
            settings.apply(ConfigUpdate {
                add_pairs: vec![existing],
                ..ConfigUpdate::default()
            }),
            "PairExists",
            |error| matches!(error, ConfigError::PairExists { vault: rejected } if *rejected == existing.vault),
        );
        assert_config_error(
            self::settings(QuoteConfig::default(), vec![pair(10), pair(10)]),
            "DuplicatePair",
            |error| matches!(error, ConfigError::DuplicatePair { vault: rejected } if *rejected == existing.vault),
        );
    }

    /// Removing a vault that is neither served nor added by the same update
    /// fails with `UnknownPair` and retires nothing; removing a vault the
    /// same update adds is accepted.
    #[test]
    fn settings_reject_removal_of_unknown_pair() {
        let unknown = pair(12);
        let mut settings = served();
        assert_config_error(
            settings.apply(ConfigUpdate {
                remove_pairs: vec![unknown.vault],
                ..ConfigUpdate::default()
            }),
            "UnknownPair",
            |error| matches!(error, ConfigError::UnknownPair { vault } if *vault == unknown.vault),
        );
        assert!(
            settings.retiring.is_empty(),
            "retiring after the rejected update: got {:?}, want none",
            settings.retiring
        );
        settings
            .apply(ConfigUpdate {
                add_pairs: vec![unknown],
                remove_pairs: vec![unknown.vault],
                ..ConfigUpdate::default()
            })
            .expect("a pair added and removed by one update is accepted");
    }

    /// A token config for a mint of no pair fails with `UnknownAsset`, and two
    /// for the same mint with `DuplicateToken`.
    #[test]
    fn settings_reject_token_config_of_unknown_or_repeated_mint() {
        let configured = pair(10);
        let range = TargetRange::new(0, 10).expect("valid range");
        let unknown = Address::new_from_array([12; 32]);
        assert_config_error(
            Settings::new(
                &ConcurrencyConfig::default(),
                QuoteConfig::default(),
                vec![configured],
                vec![(unknown, ranged(range))],
            ),
            "UnknownAsset",
            |error| matches!(error, ConfigError::UnknownAsset { mint } if *mint == unknown),
        );
        assert_config_error(
            Settings::new(
                &ConcurrencyConfig::default(),
                QuoteConfig::default(),
                vec![configured],
                vec![
                    (configured.shares_mint, ranged(range)),
                    (configured.shares_mint, ranged(range)),
                ],
            ),
            "DuplicateToken",
            |error| matches!(error, ConfigError::DuplicateToken { mint } if *mint == configured.shares_mint),
        );
    }

    /// Setting, clearing or keeping one asset's range leaves the other
    /// asset's range as it was.
    #[test]
    fn range_update_keeps_other_asset() {
        let collateral = TargetRange::new(0, 10).expect("valid range");
        let shares = TargetRange::new(20, 30).expect("valid range");
        let narrowed = TargetRange::new(0, 5).expect("valid range");
        let configured = pair(10);
        let mut settings = Settings::new(
            &ConcurrencyConfig::default(),
            QuoteConfig::default(),
            vec![configured],
            vec![
                (configured.token_mint, ranged(collateral)),
                (configured.shares_mint, ranged(shares)),
            ],
        )
        .expect("valid settings");
        settings
            .apply(ConfigUpdate {
                ranges: vec![RangeUpdate {
                    asset: configured.token_mint,
                    range: RangeChange::Set(narrowed),
                }],
                ..ConfigUpdate::default()
            })
            .expect("range update applies");
        assert_eq!(settings.range(&configured.token_mint), Some(narrowed));
        assert_eq!(settings.range(&configured.shares_mint), Some(shares));
        settings
            .apply(ConfigUpdate {
                ranges: vec![RangeUpdate {
                    asset: configured.shares_mint,
                    range: RangeChange::Clear,
                }],
                ..ConfigUpdate::default()
            })
            .expect("range update applies");
        assert_eq!(settings.range(&configured.token_mint), Some(narrowed));
        assert_eq!(settings.range(&configured.shares_mint), None);
        settings
            .apply(ConfigUpdate {
                ranges: vec![RangeUpdate {
                    asset: configured.token_mint,
                    range: RangeChange::Keep,
                }],
                ..ConfigUpdate::default()
            })
            .expect("range update applies");
        assert_eq!(settings.range(&configured.token_mint), Some(narrowed));
    }

    /// A range for an asset of a pair added in the same update applies, while
    /// a range for an asset of no pair fails with `UnknownAsset`.
    #[test]
    fn range_for_pair_added_in_same_update_is_accepted() {
        let added = pair(11);
        let range = TargetRange::new(1, 2).expect("valid range");
        let mut settings = served();
        settings
            .apply(ConfigUpdate {
                ranges: vec![RangeUpdate {
                    asset: added.shares_mint,
                    range: RangeChange::Set(range),
                }],
                add_pairs: vec![added],
                ..ConfigUpdate::default()
            })
            .expect("a range for a pair added in the same update applies");
        assert_eq!(settings.pair(&added.vault), Some(&added));
        assert_eq!(settings.range(&added.shares_mint), Some(range));
        assert_eq!(settings.range(&added.token_mint), None);
        let unknown = Address::new_from_array([12; 32]);
        assert_config_error(
            settings.apply(ConfigUpdate {
                ranges: vec![RangeUpdate {
                    asset: unknown,
                    range: RangeChange::Set(range),
                }],
                ..ConfigUpdate::default()
            }),
            "UnknownAsset",
            |error| matches!(error, ConfigError::UnknownAsset { mint } if *mint == unknown),
        );
    }

    /// A range is per asset: setting it on the shared collateral mint applies
    /// to both pairs quoting that mint and to no share mint.
    #[test]
    fn range_update_by_asset_applies_to_both_pairs_sharing_it() {
        let first = pair(10);
        let second = pair(11);
        assert_eq!(first.token_mint, second.token_mint);
        let range = TargetRange::new(3, 7).expect("valid range");
        let mut settings = settings(QuoteConfig::default(), vec![first, second])
            .expect("two pairs sharing a collateral mint are valid");
        assert_eq!(settings.assets().len(), 3);
        settings
            .apply(ConfigUpdate {
                ranges: vec![RangeUpdate {
                    asset: COLLATERAL_MINT,
                    range: RangeChange::Set(range),
                }],
                ..ConfigUpdate::default()
            })
            .expect("range update applies");
        for pair in [first, second] {
            assert_eq!(settings.range(&pair.token_mint), Some(range));
            assert_eq!(settings.range(&pair.shares_mint), None);
        }
    }
}

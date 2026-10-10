//! A market maker for private kVault deposits: it quotes swaps between a
//! vault's token and its shares, fills them from a shielded inventory in one
//! transaction with the user's own shielded transfer, and keeps that
//! inventory in range by depositing into and withdrawing from the vault.

mod api;
mod config;
mod error;
mod inventory;
mod swap;
mod transactions;

pub use self::{
    api::{Holdings, MarketMaker, VaultOperation},
    config::{
        ConcurrencyConfig, ConfigError, ConfigUpdate, ConnectionConfig, IdentityConfig,
        MarketMakerConfig, QuoteConfig, RangeChange, RangeUpdate, TargetRange, TokenConfig,
    },
    error::MakerError,
    inventory::{
        balance::{profile::InventoryProfile, reservations::InventoryUtxo},
        consolidate::ConsolidateReceipt,
    },
    swap::fill::{MakerFill, SWAP_COMPUTE_BUDGET},
    transactions::{
        budget::MAX_COMPUTE_UNITS,
        shield::{vault_compute_units, SHIELD_MARGIN_BPS, SWEEP_CAP_BPS},
    },
};

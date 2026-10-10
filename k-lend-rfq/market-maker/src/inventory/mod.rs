//! The market maker's shielded inventory: balances and reservations, the UTXO
//! profile it is split toward, and the operations that change it outside
//! swaps (seeding, consolidation, rebalancing through the vault).

pub mod balance;
pub mod consolidate;
pub mod rebalance;
pub mod setup;

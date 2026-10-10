//! The market maker's transaction pipeline: sizing, building, proving, sending
//! and confirming steps, all driven by the coordinator.

pub mod budget;
pub mod confirm;
pub mod coordinator;
pub mod kvault;
pub mod prove;
pub mod send;
pub mod shield;
pub mod steps;
pub mod transfer;

use std::sync::Arc;

use solana_address::Address;
use solana_signer::Signer;
use zolana_client::ProofAuthority;
use zolana_keypair::ShieldedAddress;
use zolana_transaction::ShieldedKeys;

/// The market maker's roles (see `IdentityConfig`) with the addresses derived
/// from them once at start.
#[derive(Clone)]
pub struct Identity {
    pub keys: Arc<dyn ShieldedKeys + Send + Sync>,
    pub authority: Arc<dyn ProofAuthority>,
    pub signer: Arc<dyn Signer + Send + Sync>,
    /// The market maker's shielded address, from `keys`.
    pub own: ShieldedAddress,
    /// The signer's public key: fee payer and public account owner.
    pub payer: Address,
    /// The state tree the market maker's UTXOs live in.
    pub tree: Address,
    /// The zolana id of `tree`.
    pub tree_id: u16,
}

use anyhow::Result;
use rust_client_example::{setup, SetupContext};
use solana_signer::Signer;
use zolana_client::SolanaRpc;
use zolana_wallet::ensure_registered;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url, sender, ..
    } = setup()?;

    // Connect to devnet.
    let rpc = SolanaRpc::new(rpc_url);

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender_solana_keypair = sender.to_solana_keypair()?;

    // Create a private wallet. This registers the Solana address
    // so others can send private transfers to it.
    ensure_registered(&rpc, &sender_solana_keypair, &sender)?;

    println!(
        "ok private wallet solana_address={}",
        sender_solana_keypair.pubkey()
    );
    Ok(())
}

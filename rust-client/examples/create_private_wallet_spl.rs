use anyhow::Result;
use rust_client_example::{setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_wallet::{ensure_registered, is_wallet_registered_sync};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
        sender,
        ..
    } = setup()?;

    // Load the funded fee payer and devnet settings, then connect.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        prover_url,
        tree,
    );

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender_solana_keypair = sender.to_solana_keypair()?;

    // Create a private wallet. This registers inbox -> shielded_public_key in the protocol registry.
    ensure_registered(&client, &sender_solana_keypair, &sender)?;

    assert!(is_wallet_registered_sync(
        &client,
        sender_solana_keypair.pubkey(),
    )?);

    println!(
        "ok private wallet solana_address={}",
        sender_solana_keypair.pubkey()
    );
    Ok(())
}

use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_wallet::{build_registration_transaction_sync, is_wallet_registered_sync};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        prover_url,
        tree,
    );

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;

    // The SDK hands back a transaction; the app owns signing and sending.
    if let Some(mut registration) = build_registration_transaction_sync(
        &client,
        sender.pubkey(),
        &sender.shielded_address()?,
        None,
    )? {
        let blockhash = registration.message.recent_blockhash;
        registration.try_sign(&[&sender], blockhash)?;
        client.send_transaction(&registration)?;
    }

    assert!(is_wallet_registered_sync(&client, sender.pubkey())?);

    println!("ok private wallet solana_address={}", sender.pubkey());
    Ok(())
}

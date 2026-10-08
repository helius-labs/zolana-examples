use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use zolana_client::user_registry::{
    build_registration_transaction_sync, is_wallet_registered_sync,
};
use zolana_client::{sign_transaction, Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;

const FUND_LAMPORTS: u64 = 10_000_000;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Load the funded fee payer and devnet settings, then connect.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&Keypair::new())?;

    let payer = cli_keypair()?;
    client.create_and_send_transaction(
        &[transfer(&payer.pubkey(), &sender.pubkey(), FUND_LAMPORTS)],
        payer.pubkey(),
        &[&payer],
        client.compute_budget(),
    )?;

    // Create a private wallet. This registers inbox -> shielded_public_key in the protocol registry.
    if let Some(registration) = build_registration_transaction_sync(
        &client,
        sender.pubkey(),
        &sender.shielded_address()?,
        None,
        None,
    )? {
        let registration = sign_transaction(registration, &[&sender])?;
        client.process_transaction(registration)?;
    }

    assert!(is_wallet_registered_sync(&client, sender.pubkey())?);

    println!("ok private wallet solana_address={}", sender.pubkey());
    Ok(())
}

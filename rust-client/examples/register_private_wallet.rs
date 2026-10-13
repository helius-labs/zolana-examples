use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use zolana_client::user_registry::is_wallet_registered_sync;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_user_registry_interface::{
    instruction::{register, set_merging_enabled, RegisterData},
    user_record_pda,
};

const FUND_LAMPORTS: u64 = 10_000_000;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&Keypair::new())?;

    // The SDK hands back a message; the CLI functions as sponsor to sign and send.
    let payer = cli_keypair()?;
    client.create_and_send_transaction(
        &[transfer(&payer.pubkey(), &sender.pubkey(), FUND_LAMPORTS)],
        payer.pubkey(),
        &[&payer],
        client.compute_budget(),
    )?;
    let sender_address = sender.shielded_address()?;
    let user_record = user_record_pda(&sender.pubkey()).0;
    client.create_and_send_transaction(
        &[
            register(
                user_record,
                sender.pubkey(),
                sender.pubkey(),
                RegisterData {
                    owner_p256: None,
                    nullifier_pubkey: sender_address.nullifier_pubkey,
                    viewing_pubkey: *sender_address.viewing_pubkey.as_bytes(),
                },
            ),
            set_merging_enabled(user_record, sender.pubkey(), true),
        ],
        sender.pubkey(),
        &[&sender],
        client.compute_budget(),
    )?;

    assert!(is_wallet_registered_sync(&client, sender.pubkey())?);

    println!("ok private wallet solana_address={}", sender.pubkey());
    Ok(())
}

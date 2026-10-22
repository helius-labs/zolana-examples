use anyhow::Result;
use rust_client_example::{cli_keypair, setup};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use zolana_client::user_registry::is_wallet_registered_sync;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_user_registry_interface::{
    instruction::{register, set_merge_encryption, RegisterData},
    user_record_pda, MergeEncryptionScheme,
};

const FUND_LAMPORTS: u64 = 10_000_000;

fn main() -> Result<()> {
    let context = setup()?;
    let client = context.client()?;
    run(&client, &cli_keypair()?)
}

pub fn run(client: &ZolanaClient<SolanaRpc>, payer: &Keypair) -> Result<()> {
    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&Keypair::new())?;

    // The SDK hands back a message; the CLI functions as sponsor to sign and send.
    client.create_and_send_transaction(
        &[transfer(&payer.pubkey(), &sender.pubkey(), FUND_LAMPORTS)],
        payer.pubkey(),
        &[payer],
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
                    nullifier_pubkey: sender_address.nullifier_pubkey,
                    viewing_pubkey: *sender_address.viewing_pubkey.as_bytes(),
                },
            ),
            set_merge_encryption(
                user_record,
                sender.pubkey(),
                MergeEncryptionScheme::PoseidonCtrV1,
            ),
        ],
        sender.pubkey(),
        &[&sender],
        client.compute_budget(),
    )?;

    assert!(is_wallet_registered_sync(client, sender.pubkey())?);

    println!("ok private wallet solana_address={}", sender.pubkey());
    Ok(())
}

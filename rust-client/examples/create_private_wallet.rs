use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_wallet::{ensure_registered, is_wallet_registered_sync};

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

    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;

    // Create a private wallet. This registers inbox -> shielded_public_key in the protocol registry.
    ensure_registered(&client, &sender, &sender)?;

    assert!(is_wallet_registered_sync(&client, sender.pubkey())?);

    println!("ok private wallet solana_address={}", sender.pubkey());
    Ok(())
}

use anyhow::Result;
use rust_client_example::setup_localnet;
use solana_keypair::Keypair;
use solana_signer::Signer;
use zolana_client::ensure_registered;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet};

fn main() -> Result<()> {
    let (mut client, _localnet) = setup_localnet()?;

    // The keypair owns and decrypts the private balance.
    let keypair = ShieldedKeypair::new()?;

    // Fund the Solana key that pays fees; registration below spends from it.
    let funding = Keypair::new();
    client.rpc.airdrop(&funding.pubkey(), 1_000_000_000)?;

    // The wallet holds the party's private balance.
    let _wallet = Wallet::new(keypair.clone(), AssetRegistry::default())?;

    // Register private wallet address for private transfers; senders transfer privately
    // to the regular Solana public key that serves as inbox for the private wallet. If a
    // recipient has no private wallet, meaning a public key is not registered, transfers
    // resolve to a private-to-public withdrawal.
    ensure_registered(&client.rpc, &funding, &keypair)?;

    println!("ok private wallet solana_address={}", funding.pubkey());
    Ok(())
}

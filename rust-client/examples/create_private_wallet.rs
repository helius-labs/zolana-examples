use anyhow::Result;
use rust_client_example::{fund_key, setup};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use zolana_client::{ensure_registered, Rpc};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet};
use zolana_user_registry_interface::user_registry_program_id;

fn main() -> Result<()> {
    let (mut client, _localnet) = setup()?;

    // The keypair owns and decrypts the private balance.
    let keypair = ShieldedKeypair::new()?;

    // Fund the Solana key that pays fees; registration below spends from it.
    let funding = Keypair::new();
    fund_key(&mut client, &funding.pubkey(), 20_000_000)?;

    // The wallet holds the party's private balance.
    let _wallet = Wallet::new(keypair.clone(), AssetRegistry::default())?;

    // Register private wallet address for private transfers; senders transfer privately
    // to the regular Solana public key that serves as inbox for the private wallet. If a
    // recipient has no private wallet, meaning a public key is not registered, transfers
    // resolve to a private-to-public withdrawal. Skip where the user-registry program is
    // not deployed.
    let registry_id = Address::new_from_array(user_registry_program_id().to_bytes());
    let registry_deployed = client
        .rpc
        .get_account(registry_id)?
        .map(|a| a.executable)
        .unwrap_or(false);
    if registry_deployed {
        ensure_registered(&client.rpc, &funding, &keypair)?;
    } else {
        eprintln!("note: user-registry program not deployed; skipping registration");
    }

    println!("ok private wallet solana_address={}", funding.pubkey());
    Ok(())
}

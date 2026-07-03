use anyhow::Result;
use rust_client_example::{deposit_sol, setup_localnet, setup_private_wallet};
use zolana_client::Rpc;

fn main() -> Result<()> {
    let (mut client, localnet) = setup_localnet()?;
    let (keypair, _funding, mut wallet) = setup_private_wallet(&mut client, &localnet)?;

    // Setup: Deposit SOL to private balance
    deposit_sol(&mut client, &keypair, &mut wallet, 5_000_000)?;
    deposit_sol(&mut client, &keypair, &mut wallet, 2_000_000)?;

    // Query indexer for private balances of a wallet and decrypts the results
    let tags = vec![keypair.recipient_bootstrap_view_tag()];
    let response = client
        .indexer
        .get_encrypted_utxos_by_tags(tags, None, None)?;

    println!("ok query encrypted_matches={}", response.matches.len());
    Ok(())
}

use anyhow::Result;
use rust_client_example::{deposit_sol, new_party, setup_localnet};
use zolana_client::Rpc;

fn main() -> Result<()> {
    let (mut client, localnet) = setup_localnet()?;
    let (sender_keypair, _sender_funding, mut sender_wallet) = new_party(&mut client, &localnet)?;

    // Deposit SOL to private balance
    deposit_sol(&mut client, &sender_keypair, &mut sender_wallet, 5_000_000)?;
    deposit_sol(&mut client, &sender_keypair, &mut sender_wallet, 2_000_000)?;

    // Query indexer for private balances of a wallet; sync_wallet wraps this lookup
    // and decrypts the results into a spendable balance
    let tags = vec![sender_keypair.recipient_bootstrap_view_tag()];
    let response = client
        .indexer
        .get_encrypted_utxos_by_tags(tags, None, None)?;

    println!("ok query encrypted_matches={}", response.matches.len());
    Ok(())
}

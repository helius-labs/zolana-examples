use anyhow::Result;
use rust_client_example::{ensure_spl_asset, setup_localnet, setup_private_wallet};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{create_deposit, get_private_token_balances, sync_wallet, CreateDeposit, Rpc};
use zolana_test_utils::{spl::mint_to, test_validator_asserts::wait_for_indexed_transaction};

fn main() -> Result<()> {
    // Register SPL and Token 2022 mints for private balances and transactions
    let (mut client, mut localnet) = setup_localnet()?;
    let asset = ensure_spl_asset(&mut client, &mut localnet)?;
    let (keypair, _funding, mut wallet) = setup_private_wallet(&mut client, &localnet)?;

    // Fund token account for deposit to private balance
    let payer = client.payer.insecure_clone();
    mint_to(&client.rpc, &payer, &asset.mint, &asset.user_token, 10_000)?;

    // Move the tokens into the private balance
    let prepared = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()),
        amount: 10_000,
        spl_token_account: Some(asset.user_token),
        memo: None,
    })?;
    let ix = prepared.instruction(client.tree, payer.pubkey());
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    let signature = client
        .rpc
        .create_and_send_transaction(&[ix], payer_address, &[&payer])?;

    // Let indexer catch up for sync of private balances
    wait_for_indexed_transaction(&client.indexer, prepared.view_tag(), signature);
    sync_wallet(&mut wallet, &client.indexer)?;
    let balance = get_private_token_balances(&wallet)?;

    println!("ok deposit signature={signature} private_balance={balance:?}");
    Ok(())
}

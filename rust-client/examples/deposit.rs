use anyhow::Result;
use rust_client_example::{register_asset, setup, setup_private_wallet};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{create_deposit, get_private_token_balances, sync_wallet, CreateDeposit, Rpc};
use zolana_test_utils::spl::mint_to;
use zolana_transaction::SOL_MINT;

fn main() -> Result<()> {
    let (mut client, mut localnet) = setup()?;
    let asset = register_asset(&mut client, &mut localnet)?;
    let (keypair, _funding, mut wallet) = setup_private_wallet(&mut client, &localnet)?;

    let payer = client.payer.insecure_clone();
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());

    // Deposit SOL to private balance
    let sol = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_ix = sol.instruction(client.tree, payer.pubkey());
    let sol_sig = client
        .rpc
        .create_and_send_transaction(&[sol_ix], payer_address, &[&payer])?;

    // Deposit an SPL token to private balance
    mint_to(&client.rpc, &payer, &asset.mint, &asset.user_token, 10_000)?;
    let spl = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()),
        amount: 10_000,
        spl_token_account: Some(asset.user_token),
        memo: None,
    })?;
    let spl_ix = spl.instruction(client.tree, payer.pubkey());
    let spl_sig = client
        .rpc
        .create_and_send_transaction(&[spl_ix], payer_address, &[&payer])?;

    // Sync the private balance, which now holds both assets.
    sync_wallet(&mut wallet, &client.indexer)?;
    let balance = get_private_token_balances(&wallet)?;

    println!("ok deposit sol_signature={sol_sig} spl_signature={spl_sig} private_balance={balance:?}");
    Ok(())
}

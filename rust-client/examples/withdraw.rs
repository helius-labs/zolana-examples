use anyhow::Result;
use rust_client_example::{
    authority, client, create_test_recipient_token_account, env_config, setup_funded_wallet,
};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{
    create_withdrawal, get_private_token_balances, sign_private_transaction_sync, sync_wallet, Rpc,
    WithdrawalParams,
};
use zolana_keypair::ShieldedKeypair;

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = client(&cfg);
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register a test asset and fund a private wallet.
    let mut sender = setup_funded_wallet(&client, &cfg.payer, &keypair, 10_000)?;
    let mint = Address::new_from_array(sender.asset.mint.to_bytes());

    // Open a public token account for the recipient: the owner or any third party.
    let (recipient, token_account) =
        create_test_recipient_token_account(&client, &cfg.payer, &sender.asset.mint)?;

    // Build the private-to-public withdrawal.
    let created = create_withdrawal(WithdrawalParams {
        wallet: &sender.wallet,
        payer: Address::new_from_array(cfg.payer.pubkey().to_bytes()),
        recipient: recipient.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
    })?;

    // Sign the withdrawal (its proof is generated during the build), then send
    // and confirm it.
    let sender_authority = authority(&cfg.payer, &keypair);
    let tx = sign_private_transaction_sync(
        created.transaction,
        &sender.wallet,
        &sender_authority,
        &client,
        &cfg.payer,
    )?;
    let signature = client.send_transaction(&tx)?;
    client.confirm_private_transaction_sync(signature)?;

    // Sync the private balance and read what remains.
    sync_wallet(&mut sender.wallet, &sender_authority, &client)?;
    let balance = get_private_token_balances(&sender.wallet)?;

    println!(
        "ok withdrawal signature={signature} recipient_token_account={token_account} remaining_private_balance={balance:?}"
    );
    Ok(())
}

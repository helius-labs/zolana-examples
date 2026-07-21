use anyhow::Result;
use rust_client_example::{env_config, setup_funded_wallet};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_withdrawal, get_private_token_balances,
    sign_private_transaction_sync, sync_wallet, Rpc, SolanaRpc, WithdrawalParams, ZolanaClient,
};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::LocalWalletAuthority;

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register a test asset and fund a private wallet.
    let mut sender = setup_funded_wallet(&client, &cfg.payer, &keypair, 10_000)?;
    let mint = Address::new_from_array(sender.asset.mint.to_bytes());

    // 1. Build the withdrawal. Open a public token account for the recipient:
    // the owner or any third party.
    let recipient = Keypair::new();
    let (_signature, token_account) = create_associated_token_account(
        &client,
        &cfg.payer,
        &recipient.pubkey(),
        &sender.asset.mint,
    )?;

    let created = create_withdrawal(WithdrawalParams {
        wallet: &sender.wallet,
        payer: Address::new_from_array(cfg.payer.pubkey().to_bytes()),
        recipient: recipient.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
    })?;

    // 2. Sign the withdrawal. Includes the proof that the sender owns and can
    // spend the balance.
    let sender_authority = LocalWalletAuthority::new(
        Address::new_from_array(cfg.payer.pubkey().to_bytes()),
        &keypair,
    );
    let tx = sign_private_transaction_sync(
        created.transaction,
        &sender.wallet,
        &sender_authority,
        &client,
        &cfg.payer,
    )?;

    // 3. Send and confirm like any Solana transaction.
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

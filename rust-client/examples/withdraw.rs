use anyhow::Result;
use rust_client_example::{env_config, setup_funded_sol_wallet, DEFAULT_RECIPIENT};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::SOL_MINT;
use zolana_wallet::{
    create_withdrawal, get_private_token_balances, sign_private_transaction_sync, sync_wallet,
    LocalWalletAuthority, WithdrawalLeg, WithdrawalParams,
};

fn main() -> Result<()> {
    // Load the funded fee payer and network settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register the sender and fund its private SOL balance.
    let mut sender_wallet = setup_funded_sol_wallet(&client, &cfg.payer, &keypair, 1_000_000_000)?;

    // Withdraw SOL from the sender's private balance to a public balance.
    // A withdrawal reveals sender, recipient, asset, and amount.

    // 1. Build the private-to-public withdrawal. The recipient can be the
    // owner or any third party.
    let recipient: Pubkey = DEFAULT_RECIPIENT.parse()?;
    let created = create_withdrawal(WithdrawalParams {
        wallet: &sender_wallet,
        payer: cfg.payer.pubkey(),
        legs: vec![WithdrawalLeg {
            recipient,
            asset: SOL_MINT,
            amount: 300_000_000,
            spl_token_program: None,
        }],
    })?;

    // 2. Sign the withdrawal. Includes the proof that the sender owns and can
    // spend the balance; signing encrypts the remaining private change.
    let sender_authority = LocalWalletAuthority::new(cfg.payer.pubkey(), &keypair);
    let tx = sign_private_transaction_sync(
        created.transaction,
        &sender_wallet,
        &sender_authority,
        &client,
        &cfg.payer,
    )?;

    // 3. Send and confirm like any Solana transaction.
    let signature = client.send_transaction(&tx)?;
    client.confirm_private_transaction_sync(signature)?;

    // 4. Sync the sender's wallet and read the remaining private balance.
    sync_wallet(&mut sender_wallet, &sender_authority, &client)?;
    let balance = get_private_token_balances(&sender_wallet)?;

    println!(
        "ok withdrawal signature={signature} recipient={recipient} remaining_private_balance={balance:?}"
    );
    Ok(())
}

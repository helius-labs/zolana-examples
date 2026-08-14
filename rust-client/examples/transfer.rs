use anyhow::Result;
use rust_client_example::{env_config, setup_funded_sol_wallet, DEFAULT_RECIPIENT};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::SOL_MINT;
use zolana_wallet::{
    create_transfer_sync, get_private_token_balances, sign_private_transaction_sync, sync_wallet,
    LocalWalletAuthority, TransferParams, TransferRecipient,
};

fn main() -> Result<()> {
    // Load the funded fee payer and network settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register the sender and fund its private SOL balance.
    let mut sender_wallet =
        setup_funded_sol_wallet(&client, &cfg.payer, &sender_keypair, 1_000_000_000)?;
    let recipient: Pubkey = DEFAULT_RECIPIENT.parse()?;
    // localnet: let recipient = rust_client_example::create_test_recipient(&client, &cfg.payer)?;

    // Confidential SOL transfer to the recipient's private balance.
    // A confidential transfer reveals only sender and recipient, not the asset
    // or amount.

    // 1. Build the transfer. The Rust action resolves the recipient's private
    // wallet by Solana pubkey; if the recipient is not registered it explicitly
    // routes the payment as a public withdrawal.
    let created = create_transfer_sync(TransferParams {
        rpc: &client,
        wallet: &sender_wallet,
        payer: cfg.payer.pubkey(),
        recipient,
        asset: SOL_MINT,
        amount: 300_000_000,
    })?;
    let routed = match &created.recipient {
        TransferRecipient::Registered(_) => "private-transfer",
        TransferRecipient::PublicWithdrawal { .. } => "public-withdrawal",
    };

    // 2. Sign the transfer. Includes the proof that the sender owns and can
    // spend the balance.
    let sender_authority = LocalWalletAuthority::new(cfg.payer.pubkey(), &sender_keypair);
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
        "ok private transfer signature={signature} routed_as={routed} remaining_private_balance={balance:?}"
    );
    Ok(())
}

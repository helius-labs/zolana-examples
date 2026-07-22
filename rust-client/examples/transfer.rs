use anyhow::Result;
use rust_client_example::{create_test_recipient, env_config, setup_funded_wallet};
use solana_signer::Signer;
use zolana_client::{
    create_transfer_sync, get_private_token_balances, sign_private_transaction_sync, sync_wallet,
    Rpc, SolanaRpc, TransferParams, TransferRecipient, ZolanaClient,
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
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register a test asset, fund a sender wallet, and create a
    // registered recipient.
    let sender = setup_funded_wallet(&client, &cfg.payer, &sender_keypair, 10_000)?;
    let mut recipient = create_test_recipient(&client, &cfg.payer, sender.registry)?;

    // 1. Build the transfer. The client resolves the recipient's private
    // wallet by pubkey; if the recipient is not registered it falls back to a
    // private-to-public withdrawal.
    let created = create_transfer_sync(TransferParams {
        rpc: &client,
        wallet: &sender.wallet,
        payer: cfg.payer.pubkey(),
        recipient: recipient.keypair.pubkey(),
        asset: sender.asset.mint, // for SOL: SOL_MINT
        amount: 4_000,
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
        &sender.wallet,
        &sender_authority,
        &client,
        &cfg.payer,
    )?;

    // 3. Send and confirm like any Solana transaction.
    let signature = client.send_transaction(&tx)?;
    client.confirm_private_transaction_sync(signature)?;

    // Sync the recipient's private balance.
    let recipient_authority =
        LocalWalletAuthority::new(recipient.keypair.pubkey(), &recipient.shielded_keypair);
    sync_wallet(&mut recipient.wallet, &recipient_authority, &client)?;
    let balance = get_private_token_balances(&recipient.wallet)?;

    println!(
        "ok private transfer signature={signature} routed_as={routed} recipient_private_balance={balance:?}"
    );
    Ok(())
}

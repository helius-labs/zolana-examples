use anyhow::Result;
use rust_client_example::{authority, client, create_test_recipient, env_config, setup_funded_wallet};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{
    create_transfer_sync, get_private_token_balances, sign_private_transaction_sync, sync_wallet,
    Rpc, TransferParams, TransferRecipient,
};
use zolana_keypair::ShieldedKeypair;

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = client(&cfg);
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register a test asset, fund a sender wallet, and create a
    // registered recipient.
    let sender = setup_funded_wallet(&client, &cfg.payer, &sender_keypair, 10_000)?;
    let mut recipient = create_test_recipient(&client, &cfg.payer, sender.registry)?;
    let mint = Address::new_from_array(sender.asset.mint.to_bytes());

    // Build the private transfer. The client resolves the recipient's private
    // wallet by pubkey; if the recipient is not registered it falls back to a
    // private-to-public withdrawal.
    let created = create_transfer_sync(TransferParams {
        rpc: &client,
        wallet: &sender.wallet,
        payer: Address::new_from_array(cfg.payer.pubkey().to_bytes()),
        recipient: recipient.keypair.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
    })?;
    let routed = match &created.recipient {
        TransferRecipient::Registered(_) => "private-transfer",
        TransferRecipient::PublicWithdrawal { .. } => "public-withdrawal",
    };

    // Sign the transfer (its proof is generated during the build), then send and
    // confirm it. Custody hosts managing many wallets scope the authority per
    // user.
    let sender_authority = authority(&cfg.payer, &sender_keypair);
    let tx = sign_private_transaction_sync(
        created.transaction,
        &sender.wallet,
        &sender_authority,
        &client,
        &cfg.payer,
    )?;
    let signature = client.send_transaction(&tx)?;
    client.confirm_private_transaction_sync(signature)?;

    // Sync the recipient's private balance. (Transfer memos were removed, so
    // there is no per-transfer note to read.)
    let recipient_authority = authority(&recipient.keypair, &recipient.shielded_keypair);
    sync_wallet(&mut recipient.wallet, &recipient_authority, &client)?;
    let balance = get_private_token_balances(&recipient.wallet)?;

    println!(
        "ok private transfer signature={signature} routed_as={routed} recipient_private_balance={balance:?}"
    );
    Ok(())
}

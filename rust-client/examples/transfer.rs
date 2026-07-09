use anyhow::Result;
use rust_client_example::{create_test_recipient, env_config, setup_funded_wallet};
use solana_signer::Signer;
use zolana_client::{
    get_private_token_balances, resolve_recipient, sync_wallet, PrivateTransfer, ZolanaClient,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let sender_keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Setup: Create test mint with interface PDA for private balances and transactions,
    // create a private wallet and test recipient.
    let sender = setup_funded_wallet(&rpc, &payer, rpc.tree(), &sender_keypair, 10_000)?;
    let mut recipient = create_test_recipient(&rpc, &payer, sender.registry)?;
    let mint = sender.asset.mint;

    // Fetch the recipient's private wallet `inbox`. If the recipient does not have a private wallet,
    // the transfer resolves to a private-to-public withdrawal.
    let recipient_address = resolve_recipient(&rpc, recipient.keypair.pubkey())?;

    // Build and sign the private transfer. Custody hosts managing many
    // wallets scope the authority per user and finish with `.create().await`.
    let transfer = PrivateTransfer {
        source: &sender.wallet,
        destination: recipient_address,
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
        authority: &sender_keypair,
        payer: payer.pubkey(),
        memo: Some(b"thanks".to_vec()), // optional encrypted memo for recipient
    }
    .instruction()?;

    // Prove and send the private transfer. The proof shows the sender owns
    // the balance being spent and has not already spent it.
    let signature = rpc.send(&payer).execute(&transfer)?;

    // Sync the recipient's private balance and decrypt memo.
    sync_wallet(&mut recipient.wallet, &rpc)?;
    let balance = get_private_token_balances(&recipient.wallet)?;
    let memo = recipient.wallet.last_memo().unwrap_or_default();

    println!(
        "ok private transfer signature={signature} recipient_private_balance={balance:?} memo={memo}"
    );
    Ok(())
}

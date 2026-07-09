use anyhow::Result;
use rust_client_example::{create_test_recipient, env_config, setup_funded_wallet};
use solana_signer::Signer;
use zolana_client::{resolve_recipient, sync_wallet, PrivateTransfer, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    // One ed25519 key signs both the Solana account and the private balance.
    let sender_keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Test setup: a test asset, the sender's funded private wallet, and the
    // recipient's private wallet.
    let sender = setup_funded_wallet(&rpc, &payer, rpc.tree(), &sender_keypair, 10_000)?;
    let mut recipient = create_test_recipient(&rpc, &payer, sender.registry)?;
    let mint = sender.asset.mint;

    // Resolve the recipient's private wallet address: one chain read. If the
    // recipient does not have a private wallet, the transfer resolves to a
    // private-to-public withdrawal.
    let recipient_address = resolve_recipient(&rpc, recipient.keypair.pubkey())?;

    // Build and sign the private transfer. Local only, no network.
    let transfer = PrivateTransfer {
        wallet: &sender.wallet,
        authority: &sender_keypair,
        owner_pubkey: None, // local key: the authority holds one wallet
        payer: payer.pubkey(),
        recipient: recipient_address,
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
        memo: Some(b"thanks".to_vec()), // encrypted; only the recipient can read it
    }
    .instruction()?;

    // Prove and send the private transfer. The proof shows the sender owns
    // the balance being spent and has not already spent it.
    let signature = rpc.send(&payer).execute(&transfer)?;

    // Sync the recipient's private balance; the memo arrives with it, decrypted.
    sync_wallet(&mut recipient.wallet, &rpc)?;
    let balance = recipient.wallet.private_token_balances()?;
    let memo = recipient.wallet.last_memo().unwrap_or_default();

    println!(
        "ok private transfer signature={signature} recipient_private_balance={balance:?} memo={memo}"
    );
    Ok(())
}

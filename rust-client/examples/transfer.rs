use anyhow::{anyhow, Result};
use rust_client_example::{create_test_recipient, env_config, setup_funded_wallet};
use solana_address::Address;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_transfer_sync, get_private_token_balances, sync_wallet, CreateTransfer, ZolanaClient,
};
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

    // Build and sign the private transfer. If the recipient does not have a
    // private wallet, the SDK resolves to a private-to-public withdrawal.
    let sender_address = Address::new_from_array(payer.pubkey().to_bytes());
    let mint = Address::new_from_array(sender.asset.mint.to_bytes());
    let transfer = create_transfer_sync(CreateTransfer {
        rpc: &rpc,
        wallet: &sender.wallet,
        authority: &sender_keypair,
        owner_pubkey: Pubkey::default(),
        payer: sender_address,
        recipient: recipient.keypair.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
        memo: Some(b"thanks".to_vec()), // encrypted; only the recipient can read it
    })?;
    if transfer.recipient.is_public_withdrawal() {
        return Err(anyhow!(
            "expected a private transfer, got a public withdrawal"
        ));
    }

    // Prove and submit the private transfer. The proof shows the sender owns the
    // balance being spent and has not already spent it.
    let signature = rpc.submit(&payer).execute(
        transfer.signed,
        transfer.recipient.withdrawal().cloned(),
        transfer.wait_tag,
    )?;

    // Sync the recipient's private balance. The memo arrives with it, decrypted.
    sync_wallet(&mut recipient.wallet, &rpc)?;
    let balance = get_private_token_balances(&recipient.wallet)?;
    let memo = recipient
        .wallet
        .utxos
        .iter()
        .find_map(|entry| entry.utxo.data.memo())
        .map(String::from_utf8_lossy);

    println!(
        "ok private transfer signature={signature} recipient_private_balance={balance:?} memo={memo:?}"
    );
    Ok(())
}

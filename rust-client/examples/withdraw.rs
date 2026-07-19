use anyhow::Result;
use rust_client_example::{create_test_recipient_token_account, env_config, setup_funded_wallet};
use solana_signer::Signer;
use zolana_client::{get_private_token_balances, sync_wallet, Withdrawal, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Setup: Create test mint with interface PDA for private balances and transactions,
    // create a funded private wallet.
    let mut sender = setup_funded_wallet(&rpc, &payer, rpc.tree(), &keypair, 10_000)?;
    let mint = sender.asset.mint;

    // Create a public token account for the recipient: the owner or any third party.
    let (recipient, token_account) = create_test_recipient_token_account(&rpc, &payer, &mint)?;

    // Build and sign the private-to-public withdrawal.
    let withdrawal = Withdrawal {
        source: &sender.wallet,
        destination: recipient.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
        authority: &keypair,
        payer: payer.pubkey(),
    }
    .instruction()?;

    // Generate proof that the sender owns the private balance and has not
    // already spent it. Then send the withdrawal.
    let signature = rpc.send(&payer).execute(&withdrawal)?;

    // Sync the private balance and read what remains.
    sync_wallet(&mut sender.wallet, &rpc)?;
    let balance = get_private_token_balances(&sender.wallet)?;

    println!(
        "ok withdrawal signature={signature} recipient_token_account={token_account} remaining_private_balance={balance:?}"
    );
    Ok(())
}

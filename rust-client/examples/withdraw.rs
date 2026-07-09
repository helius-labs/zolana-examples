use anyhow::Result;
use rust_client_example::{
    create_test_recipient_token_account, env_config, setup_funded_wallet, FundedWallet,
};
use solana_signer::Signer;
use zolana_client::{sync_wallet, Withdrawal, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Test setup: a test asset and the owner's funded private wallet.
    let FundedWallet {
        asset, mut wallet, ..
    } = setup_funded_wallet(&rpc, &payer, rpc.tree(), &keypair, 10_000)?;
    // Recipient for withdrawal can be owner or third party.
    let (recipient, token_account) =
        create_test_recipient_token_account(&rpc, &payer, &asset.mint)?;

    // Build and sign the private-to-public withdrawal. Local only, no network.
    let withdrawal = Withdrawal {
        wallet: &wallet,
        authority: &keypair,
        owner_pubkey: None,
        payer: payer.pubkey(),
        recipient: recipient.pubkey(),
        asset: asset.mint, // for SOL: SOL_MINT
        amount: 4_000,
    }
    .instruction()?;

    // Prove and send the withdrawal. The proof shows the sender owns the
    // balance being spent and has not already spent it.
    let signature = rpc.send(&payer).execute(&withdrawal)?;

    // Sync the private balance.
    sync_wallet(&mut wallet, &rpc)?;

    // Withdrawing SOL works the same way with `asset: SOL_MINT`.

    println!("ok withdrawal signature={signature} recipient_token_account={token_account}");
    Ok(())
}

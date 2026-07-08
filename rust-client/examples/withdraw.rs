use anyhow::Result;
use rust_client_example::{
    create_test_recipient_token_account, env_config, setup_funded_wallet, FundedWallet,
};
use solana_address::Address;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{create_withdrawal_sync, sync_wallet, CreateWithdrawal, Submit, ZolanaClient};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let client = ZolanaClient::devnet(payer, &api_key);

    // Test setup: a test asset and the owner's funded private wallet.
    let FundedWallet {
        asset, mut wallet, ..
    } = setup_funded_wallet(&client, &keypair, 10_000)?;
    // Recipient for withdrawal can be owner or third party.
    let (recipient, token_account) = create_test_recipient_token_account(&client, &asset.mint)?;

    // Build and sign the private-to-public withdrawal.
    let owner_address = Address::new_from_array(client.payer().pubkey().to_bytes());
    let mint = Address::new_from_array(asset.mint.to_bytes());
    let withdrawal = create_withdrawal_sync(CreateWithdrawal {
        wallet: &wallet,
        authority: &keypair,
        owner_pubkey: Pubkey::default(),
        payer: owner_address,
        recipient: recipient.pubkey(),
        asset: mint, // for SOL: SOL_MINT
        amount: 4_000,
    })?;

    // Prove and submit the withdrawal. The proof shows the sender owns the balance
    // being spent and has not already spent it.
    let signature = Submit {
        indexer: client.indexer(),
        rpc: client.rpc(),
        prover: client.prover(),
        payer: client.payer(),
        tree: client.tree(),
        cu_limit: None,
    }
    .execute(
        withdrawal.signed,
        Some(withdrawal.withdrawal),
        withdrawal.wait_tag,
    )?;

    // Sync the private balance.
    sync_wallet(&mut wallet, client.indexer())?;

    // Withdrawing SOL works the same way with `asset: SOL_MINT`.

    println!("ok withdrawal signature={signature} recipient_token_account={token_account}");
    Ok(())
}

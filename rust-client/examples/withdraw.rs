use anyhow::Result;
use rust_client_example::{deposit_spl, env_config, register_asset};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_private_wallet, create_withdrawal_sync, sync_wallet,
    CreateWithdrawal, Submit, ZolanaClient,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    // One ed25519 key signs both the Solana account and the private balance.
    let seed = *payer.secret_bytes();
    let client = ZolanaClient::devnet(payer, &api_key);

    // Create a test mint with an interface PDA for private balances and
    // transactions, then create a private wallet.
    let (asset, registry) = register_asset(&client)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let mut wallet =
        create_private_wallet(client.rpc(), client.payer(), keypair.clone(), registry)?;

    // Deposit the SPL asset to withdraw privately below.
    deposit_spl(&client, &keypair, &mut wallet, &asset, 10_000)?;

    // Recipient for withdrawal can be owner or third party.
    let recipient = Keypair::new();
    let (_ata_sig, ata) = create_associated_token_account(
        client.rpc(),
        client.payer(),
        &recipient.pubkey(),
        &asset.mint,
    )?;

    // Build and sign the private-to-public withdrawal.
    let owner_address = Address::new_from_array(client.payer().pubkey().to_bytes());
    let withdrawal = create_withdrawal_sync(CreateWithdrawal {
        wallet: &wallet,
        authority: &keypair,
        owner_pubkey: Pubkey::default(),
        payer: owner_address,
        recipient: recipient.pubkey(),
        asset: asset_address, // for SOL: SOL_MINT
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

    // Withdrawing SOL works the same way with `asset: SOL_MINT`; it goes to the
    // recipient's address directly, no token account needed.

    println!("ok withdrawal signature={signature} recipient_token_account={ata}");
    Ok(())
}

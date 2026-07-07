use anyhow::{anyhow, Result};
use rust_client_example::{deposit_spl, register_asset};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_private_wallet, create_withdrawal_sync, sync_wallet,
    CreateWithdrawal, Submit, ZolanaClient,
};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_keypair::{ShieldedKeypair, ViewingKey};

fn main() -> Result<()> {
    // Load .env if present.
    dotenvy::dotenv().ok();

    // Load the fee payer, then connect to devnet with one client.
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let payer_path = shellexpand::tilde(&payer_path).into_owned();
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let api_key = std::env::var("API_KEY").expect("set API_KEY");
    let mut client = ZolanaClient::devnet(payer, &api_key);
    let (rpc, indexer, prover, payer) = client.parts();
    let tree: Pubkey = std::env::var("ZOLANA_TREE")
        .expect("set ZOLANA_TREE")
        .parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Create a test mint with an interface PDA for private balances and transactions,
    // then create a private wallet. One ed25519 key signs both the Solana
    // account and the private balance.
    let (asset, registry) = register_asset(rpc, payer)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let seed = *payer.secret_bytes();
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let mut wallet = create_private_wallet(rpc, payer, keypair.clone(), registry)?;

    // Deposit the SPL asset to withdraw privately below.
    deposit_spl(
        rpc,
        payer,
        tree,
        indexer,
        &keypair,
        &mut wallet,
        &asset,
        10_000,
    )?;

    // Recipient for withdrawal can be owner or third party.
    let recipient = Keypair::new();
    let (_ata_sig, ata) =
        create_associated_token_account(rpc, payer, &recipient.pubkey(), &asset.mint)?;

    // Build and sign the private-to-public withdrawal.
    let owner_address = Address::new_from_array(payer.pubkey().to_bytes());
    let withdrawal = create_withdrawal_sync(CreateWithdrawal {
        wallet: &wallet,
        authority: &keypair,
        owner_pubkey: Pubkey::default(),
        payer: owner_address,
        recipient: recipient.pubkey(),
        asset: asset_address,
        amount: 4_000,
    })?;

    // Prove and submit the withdrawal. The proof shows the sender owns the balance
    // being spent and has not already spent it.
    let signature = Submit {
        indexer,
        rpc: &*rpc,
        prover,
        payer,
        tree,
        cu_limit: None,
    }
    .execute(
        withdrawal.signed,
        Some(withdrawal.withdrawal),
        withdrawal.wait_tag,
    )?;

    // Sync the private balance.
    sync_wallet(&mut wallet, indexer)?;

    // Withdrawing SOL works the same way with `asset: SOL_MINT`; it goes to the
    // recipient's address directly, no token account needed.

    println!("ok withdrawal signature={signature} recipient_token_account={ata}");
    Ok(())
}

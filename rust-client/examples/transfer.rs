use anyhow::{anyhow, Result};
use rust_client_example::{deposit_spl, fund_key, register_asset};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_private_wallet, create_transfer_sync, get_private_token_balances, sync_wallet,
    CreateTransfer, Submit, ZolanaClient,
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
    // then create sender and recipient wallets. One ed25519 key signs both the
    // Solana account and the private balance.
    let (asset, registry) = register_asset(rpc, payer)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let sender_seed = *payer.secret_bytes();
    let sender_keypair = ShieldedKeypair::from_ed25519(&sender_seed, ViewingKey::new())?;
    let mut sender_wallet =
        create_private_wallet(rpc, payer, sender_keypair.clone(), registry.clone())?;
    // The recipient owns and pays for its own registration, so fund it first.
    let recipient = Keypair::new();
    fund_key(rpc, payer, &recipient.pubkey(), 20_000_000)?;
    let recipient_seed = *recipient.secret_bytes();
    let recipient_keypair = ShieldedKeypair::from_ed25519(&recipient_seed, ViewingKey::new())?;
    let mut recipient_wallet = create_private_wallet(rpc, &recipient, recipient_keypair, registry)?;

    // Deposit the SPL asset to send. Only the sent asset is spent privately;
    // the Solana transaction fee is paid publicly by the payer keypair.
    deposit_spl(
        rpc,
        payer,
        tree,
        indexer,
        &sender_keypair,
        &mut sender_wallet,
        &asset,
        10_000,
    )?;

    // Build and sign the private transfer. If recipient does not have a private
    // wallet, the SDK resolves to a private-to-public withdrawal.
    let sender_address = Address::new_from_array(payer.pubkey().to_bytes());
    let transfer = create_transfer_sync(CreateTransfer {
        rpc: &*rpc,
        wallet: &sender_wallet,
        authority: &sender_keypair,
        owner_pubkey: Pubkey::default(),
        payer: sender_address,
        recipient: recipient.pubkey(),
        asset: asset_address,
        amount: 4_000,
    })?;

    // Prove and submit the private transfer. The proof shows the sender owns the
    // balance being spent and has not already spent it.
    let signature = Submit {
        indexer,
        rpc: &*rpc,
        prover,
        payer,
        tree,
        cu_limit: None,
    }
    .execute(
        transfer.signed,
        transfer.recipient.withdrawal().cloned(),
        transfer.wait_tag,
    )?;

    // Sync the recipient's private balance.
    sync_wallet(&mut recipient_wallet, indexer)?;
    let balance = get_private_token_balances(&recipient_wallet)?;

    println!("ok private transfer signature={signature} recipient_private_balance={balance:?}");
    Ok(())
}

use anyhow::{anyhow, Result};
use rust_client_example::register_asset;
use solana_address::Address;
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use zolana_client::{
    create_deposit, create_private_wallet, get_private_token_balances, CreateDeposit, ZolanaClient,
};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::mint_to;
use zolana_transaction::SOL_MINT;

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
    let (rpc, indexer, _prover, payer) = client.parts();
    let tree: Pubkey = std::env::var("ZOLANA_TREE")
        .expect("set ZOLANA_TREE")
        .parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Create test mint with interface PDA for private balances and transactions,
    // then create a private wallet. One ed25519 key signs both the Solana
    // account and the private balance.
    let (asset, registry) = register_asset(rpc, payer)?;
    let seed = *payer.secret_bytes();
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let mut wallet = create_private_wallet(rpc, payer, keypair.clone(), registry)?;
    mint_to(rpc, payer, &asset.mint, &asset.user_token, 10_000)?;

    // Deposit SOL to the private balance and wait until the wallet sees it.
    let sol = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_sig = sol.send_and_sync(rpc, payer, tree, payer, &mut wallet, indexer)?;

    // Deposit an SPL token to the private balance.
    let spl = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()),
        amount: 10_000,
        spl_token_account: Some(asset.user_token),
        memo: None,
    })?;
    let spl_sig = spl.send_and_sync(rpc, payer, tree, payer, &mut wallet, indexer)?;

    let balance = get_private_token_balances(&wallet)?;

    println!(
        "ok deposit sol_signature={sol_sig} spl_signature={spl_sig} private_balance={balance:?}"
    );
    Ok(())
}

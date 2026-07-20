use anyhow::Result;
use rust_client_example::{client, env_config, register_asset, tree_pubkey};
use solana_address::Address;
use zolana_client::{create_deposit, ensure_registered, get_private_token_balances, DepositParams};
use zolana_keypair::ShieldedKeypair;
use zolana_test_utils::spl::mint_to;
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};

fn main() -> Result<()> {
    // Load the fee payer and localnet settings, then connect.
    let cfg = env_config()?;
    let client = client(&cfg);
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let tree = tree_pubkey(&client);

    // Setup: register a test mint with its interface PDA, register the wallet,
    // and mint test tokens for the SPL deposit.
    let (asset, _registry) = register_asset(&client, &cfg.payer)?;
    ensure_registered(&client, &cfg.payer, &keypair)?;
    let mut wallet = Wallet::new(keypair.shielded_address()?, AssetRegistry::default())?;
    mint_to(&client, &cfg.payer, &asset.mint, &asset.user_token, 10_000)?;

    // Deposit SOL into the private balance.
    let sol = create_deposit(DepositParams {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_signature = sol.send(&client, &cfg.payer, tree, &cfg.payer)?;
    client.confirm_private_transaction_sync(sol_signature)?;

    // Deposit an SPL token into the private balance.
    let spl = create_deposit(DepositParams {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()), // for SOL: SOL_MINT
        amount: 10_000,
        spl_token_account: Some(asset.user_token), // for SOL: None
        memo: Some(b"deposit note".to_vec()),      // public: readable by anyone onchain
    })?;
    let spl_signature = spl.send(&client, &cfg.payer, tree, &cfg.payer)?;
    client.confirm_private_transaction_sync(spl_signature)?;

    // Sync from the indexer, then read the private balance per asset.
    let authority = rust_client_example::authority(&cfg.payer, &keypair);
    zolana_client::sync_wallet(&mut wallet, &authority, &client)?;
    let balance = get_private_token_balances(&wallet)?;

    println!(
        "ok deposit sol_signature={sol_signature} spl_signature={spl_signature} private_balance={balance:?}"
    );
    Ok(())
}

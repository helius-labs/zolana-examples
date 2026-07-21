use anyhow::Result;
use rust_client_example::{env_config, register_asset, sync_after_deposit};
use solana_address::Address;
use solana_pubkey::Pubkey;
use zolana_client::{
    create_deposit, get_private_token_balances, DepositParams, SolanaRpc, ZolanaClient,
};
use zolana_keypair::ShieldedKeypair;
use zolana_test_utils::spl::mint_to;
use zolana_transaction::{Wallet, SOL_MINT};

fn main() -> Result<()> {
    // Load the fee payer and settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Setup: register a test mint with its interface PDA and open the wallet.
    let (asset, registry) = register_asset(&client, &cfg.payer)?;
    let mut wallet = Wallet::new(keypair.shielded_address()?, registry)?;
    let tree = Pubkey::new_from_array(client.tree().to_bytes());

    // Deposit SOL into the private balance, then wait for the indexer and sync.
    let deposit = create_deposit(DepositParams {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let signature = deposit.send(&client, &cfg.payer, tree, &cfg.payer)?;
    sync_after_deposit(&client, &mut wallet, &cfg.payer, &keypair, deposit.view_tag(), signature)?;

    // Fund the token account, then deposit the SPL token the same way.
    mint_to(&client, &cfg.payer, &asset.mint, &asset.user_token, 10_000)?;
    let deposit = create_deposit(DepositParams {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()),
        amount: 10_000,
        spl_token_account: Some(asset.user_token),
        memo: None,
    })?;
    let signature = deposit.send(&client, &cfg.payer, tree, &cfg.payer)?;
    sync_after_deposit(&client, &mut wallet, &cfg.payer, &keypair, deposit.view_tag(), signature)?;

    // Read the private balance per asset.
    let balance = get_private_token_balances(&wallet)?;

    println!("ok deposit private_balance={balance:?}");
    Ok(())
}

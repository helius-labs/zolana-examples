use anyhow::Result;
use rust_client_example::{
    client, deposit_sol, deposit_spl, env_config, register_asset,
};
use zolana_client::get_private_token_balances;
use zolana_transaction::Wallet;

fn main() -> Result<()> {
    // Load the fee payer and settings, then connect.
    let cfg = env_config()?;
    let client = client(&cfg);
    let keypair = rust_client_example::shielded_keypair(&cfg.payer)?;

    // Setup: register a test mint with its interface PDA and open the wallet.
    let (asset, registry) = register_asset(&client, &cfg.payer)?;
    let mut wallet = Wallet::new(keypair.shielded_address()?, registry)?;

    // Deposit SOL, then an SPL token, into the private balance.
    deposit_sol(&client, &cfg.payer, &keypair, &mut wallet, 5_000_000)?;
    deposit_spl(&client, &cfg.payer, &keypair, &mut wallet, &asset, 10_000)?;

    // Read the private balance per asset.
    let balance = get_private_token_balances(&wallet)?;

    println!("ok deposit private_balance={balance:?}");
    Ok(())
}

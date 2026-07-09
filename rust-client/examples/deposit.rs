use anyhow::Result;
use rust_client_example::{env_config, register_asset};
use zolana_client::{
    create_deposit, create_private_wallet, get_private_token_balances, Deposit, ZolanaClient,
    SOL_MINT,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::mint_to;

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);

    // Setup: Create test mint with interface PDA for private balances and transactions,
    // create a private wallet, and mint test tokens for the SPL deposit.
    let (asset, registry) = register_asset(&rpc, &payer)?;
    let mut wallet = create_private_wallet(&rpc, &payer, keypair.clone(), registry)?;
    mint_to(&rpc, &payer, &asset.mint, &asset.user_token, 10_000)?;

    // Deposit SOL to the private balance.
    let sol = create_deposit(Deposit {
        destination: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_signature = sol.send(&rpc, &payer, rpc.tree(), &payer)?;
    sol.wait_until_synced(&mut wallet, &rpc, sol_signature)?;

    // Deposit an SPL token to the private balance.
    let spl = create_deposit(Deposit {
        destination: &keypair.shielded_address()?,
        asset: asset.mint, // for SOL: SOL_MINT
        amount: 10_000,
        spl_token_account: Some(asset.user_token), // for SOL: None
        memo: Some(b"deposit note".to_vec()),      // public: readable by anyone onchain
    })?;
    let spl_signature = spl.send(&rpc, &payer, rpc.tree(), &payer)?;
    spl.wait_until_synced(&mut wallet, &rpc, spl_signature)?;

    // Read the private balance per asset.
    let balance = get_private_token_balances(&wallet)?;

    println!(
        "ok deposit sol_signature={sol_signature} spl_signature={spl_signature} private_balance={balance:?}"
    );
    Ok(())
}

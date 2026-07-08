use anyhow::Result;
use rust_client_example::{env_config, register_asset};
use solana_address::Address;
use zolana_client::{
    create_deposit, create_private_wallet, get_private_token_balances, CreateDeposit, ZolanaClient,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::mint_to;
use zolana_transaction::SOL_MINT;

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    // One ed25519 key signs both the Solana account and the private balance.
    let seed = *payer.secret_bytes();
    let client = ZolanaClient::devnet(payer, &api_key);

    // Create test mint with interface PDA for private balances and transactions,
    // then create a private wallet.
    let (asset, registry) = register_asset(&client)?;
    let keypair = ShieldedKeypair::from_ed25519(&seed, ViewingKey::new())?;
    let mut wallet =
        create_private_wallet(client.rpc(), client.payer(), keypair.clone(), registry)?;
    mint_to(
        client.rpc(),
        client.payer(),
        &asset.mint,
        &asset.user_token,
        10_000,
    )?;

    // Deposit SOL to the private balance. The wait is the recipient-side
    // read-your-write step, not part of depositing: this example deposits to
    // its own wallet and reads the balance right after, so it blocks until
    // the wallet sees the UTXO.
    let sol = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_sig = sol.send(client.rpc(), client.payer(), client.tree(), client.payer())?;
    sol.wait_until_synced(&mut wallet, client.indexer(), sol_sig)?;

    // Deposit an SPL token to the private balance.
    let spl = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()), // for SOL: SOL_MINT
        amount: 10_000,
        spl_token_account: Some(asset.user_token), // for SOL: None
        memo: None,
    })?;
    let spl_sig = spl.send(client.rpc(), client.payer(), client.tree(), client.payer())?;
    spl.wait_until_synced(&mut wallet, client.indexer(), spl_sig)?;

    let balance = get_private_token_balances(&wallet)?;

    println!(
        "ok deposit sol_signature={sol_sig} spl_signature={spl_sig} private_balance={balance:?}"
    );
    Ok(())
}

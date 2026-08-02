use anyhow::Result;
use rust_client_example::{env_config, sync_after_deposit};
use zolana_client::{SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, Wallet, SOL_MINT};
use zolana_wallet::{create_deposit, get_private_token_balances, DepositParams};

fn main() -> Result<()> {
    // Load the funded fee payer and network settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;

    // Initialize the sender's private wallet. A wallet is local state; the
    // deposit creates its first private balance on-chain.
    let mut wallet = Wallet::new(keypair.shielded_address()?, AssetRegistry::default())?;
    let tree = cfg.tree_pubkey();

    // Deposit SOL into the sender's private balance.
    // A deposit from a public balance reveals sender, recipient, asset, and
    // amount. Alternatively, you can onramp fiat directly to a private balance.

    // 1. Build the public-to-private deposit.
    let deposit = create_deposit(DepositParams {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        spl_token_program: None,
        memo: None,
    })?;

    // 2. Send like any Solana transaction.
    let signature = deposit.send(&client, &cfg.payer, tree, &cfg.payer)?;

    // 3. Wait until Photon has indexed the output, then decrypt it locally and
    // update the private balance.
    sync_after_deposit(
        &client,
        &mut wallet,
        &cfg.payer,
        &keypair,
        deposit.view_tag(),
        signature,
    )?;

    // 4. Read the private balance per asset.
    let balance = get_private_token_balances(&wallet)?;

    println!("ok deposit signature={signature} private_balance={balance:?}");
    Ok(())
}

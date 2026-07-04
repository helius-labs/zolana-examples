use anyhow::{anyhow, Result};
use rust_client_example::{create_private_wallet, register_asset};
use solana_address::Address;
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_deposit, get_private_token_balances, sync_wallet, CreateDeposit, ProverClient, Rpc,
    SolanaRpc, ZolanaIndexer,
};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_test_utils::spl::mint_to;
use zolana_transaction::SOL_MINT;

fn main() -> Result<()> {
    // Connect to the devnet deployment.
    let indexer = ZolanaIndexer::new("http://202.8.10.77:8784/");
    let rpc_url = format!(
        "https://devnet.helius-rpc.com/?api-key={}",
        std::env::var("API_KEY").expect("set API_KEY"),
    );
    let mut rpc = SolanaRpc::new(rpc_url).with_indexer(indexer.clone());
    let _prover = ProverClient::new("http://202.8.10.77:3011".to_string());
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| format!("{}/.config/solana/id.json", std::env::var("HOME").unwrap_or_default()));
    let payer = read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree: Pubkey = std::env::var("ZOLANA_TREE").expect("set ZOLANA_TREE").parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    let (asset, registry) = register_asset(&mut rpc, &payer)?;
    let (keypair, _funding, mut wallet) = create_private_wallet(&mut rpc, &payer, registry)?;

    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());

    // Deposit SOL to private balance
    let sol = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_ix = sol.instruction(tree, payer.pubkey());
    let sol_sig = rpc.create_and_send_transaction(&[sol_ix], payer_address, &[&payer])?;

    // Deposit an SPL token to private balance
    mint_to(&rpc, &payer, &asset.mint, &asset.user_token, 10_000)?;
    let spl = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()),
        amount: 10_000,
        spl_token_account: Some(asset.user_token),
        memo: None,
    })?;
    let spl_ix = spl.instruction(tree, payer.pubkey());
    let spl_sig = rpc.create_and_send_transaction(&[spl_ix], payer_address, &[&payer])?;

    // Sync the private balance, which now holds both assets.
    sync_wallet(&mut wallet, &indexer)?;
    let balance = get_private_token_balances(&wallet)?;

    println!("ok deposit sol_signature={sol_sig} spl_signature={spl_sig} private_balance={balance:?}");
    Ok(())
}

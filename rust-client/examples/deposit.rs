use anyhow::Result;
use rust_client_example::{env_config, register_asset};
use solana_address::Address;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_signer::Signer;
use zolana_client::{
    create_deposit, create_private_wallet, get_private_token_balances, CreateDeposit, Rpc,
    ZolanaClient, DEFAULT_DEPOSIT_CU_LIMIT,
};
use zolana_keypair::{ShieldedKeypair, ViewingKey};
use zolana_test_utils::spl::mint_to;
use zolana_transaction::SOL_MINT;

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    // One ed25519 key signs both the Solana account and the private balance.
    let keypair = ShieldedKeypair::from_ed25519(&payer, ViewingKey::new())?;
    let rpc = ZolanaClient::devnet(&api_key);
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());

    // Create test mint with interface PDA for private balances and transactions,
    // then create a private wallet.
    let (asset, registry) = register_asset(&rpc, &payer)?;
    let mut wallet = create_private_wallet(&rpc, &payer, keypair.clone(), registry)?;
    mint_to(&rpc, &payer, &asset.mint, &asset.user_token, 10_000)?;

    // Deposit SOL to the private balance.
    let sol = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: SOL_MINT,
        amount: 5_000_000,
        spl_token_account: None,
        memo: None,
    })?;
    let sol_instruction = sol.instruction(rpc.tree(), payer.pubkey());
    let cu_limit = ComputeBudgetInstruction::set_compute_unit_limit(DEFAULT_DEPOSIT_CU_LIMIT);
    let sol_sig = rpc.create_and_send_transaction(
        &[cu_limit.clone(), sol_instruction],
        payer_address,
        &[&payer],
    )?;
    sol.wait_until_synced(&mut wallet, &rpc, sol_sig)?;

    // Deposit an SPL token to the private balance.
    let spl = create_deposit(CreateDeposit {
        recipient: &keypair.shielded_address()?,
        asset: Address::new_from_array(asset.mint.to_bytes()), // for SOL: SOL_MINT
        amount: 10_000,
        spl_token_account: Some(asset.user_token), // for SOL: None
        memo: Some(b"deposit note".to_vec()),      // public: readable by anyone onchain
    })?;
    let spl_instruction = spl.instruction(rpc.tree(), payer.pubkey());
    let spl_sig =
        rpc.create_and_send_transaction(&[cu_limit, spl_instruction], payer_address, &[&payer])?;
    spl.wait_until_synced(&mut wallet, &rpc, spl_sig)?;

    let balance = get_private_token_balances(&wallet)?;

    println!(
        "ok deposit sol_signature={sol_sig} spl_signature={spl_sig} private_balance={balance:?}"
    );
    Ok(())
}

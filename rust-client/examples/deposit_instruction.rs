use anyhow::{anyhow, Result};
use rust_client_example::{env_config, setup_funded_spl_asset};
use solana_address::Address;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::{
    instruction::{Deposit, DepositSplAccounts},
    pda,
};
use zolana_keypair::{random_blinding, ShieldedKeypair};
use zolana_transaction::decrypt_transactions;

fn main() -> Result<()> {
    // Load the fee payer and localnet settings.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let payer = cfg.payer.pubkey();
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let sender_address = sender_keypair.shielded_address()?;
    let sender_tag = sender_address.confidential_view_tag()?;
    let state_tree = Pubkey::new_from_array(client.tree().to_bytes());

    // Create a test mint with an interface PDA and fund the token account.
    let deposit_amount = 10_000;
    let (asset, registry) = setup_funded_spl_asset(
        &client,
        &cfg.payer,
        deposit_amount,
    )?;
    let mint = Address::new_from_array(asset.mint.to_bytes());
    let vault = pda::spl_asset_vault(&asset.mint);

    // 1. Move public SPL into the sender's private balance.
    let deposit_instruction = Deposit {
        tree: state_tree,
        depositor: payer,
        spl: Some(DepositSplAccounts {
            user_token: asset.user_token,
            spl_token_interface: vault,
            registry: pda::spl_asset_registry(&asset.mint),
            token_program: pda::spl_token_program_id(),
        }),
        view_tag: sender_tag,
        owner: sender_address.owner_hash()?,
        blinding: random_blinding(),
        amount: deposit_amount,
        utxo_data: None,
        memo: None,
    }
    .instruction();

    // 2. Send like any Solana transaction.
    let signature = client.create_and_send_transaction(
        &[deposit_instruction],
        payer,
        &[&cfg.payer],
    )?;

    // 3. Read the balance from the indexer. A deposit is a public Solana
    // transaction that reveals the asset and amount.
    let response = client.get_shielded_transactions_by_tags(
        vec![sender_tag],
        None,
        Some(50),
        Some(IndexerRpcConfig::wait()),
    )?;
    let sender_balances = decrypt_transactions(
        &sender_keypair,
        &response.transactions,
        &registry,
    )
    .map_err(|error| anyhow!("decrypt sender transactions: {error:?}"))?;
    let sender_balance = sender_balances
        .get_balance(mint)
        .ok_or_else(|| anyhow!("failed to fetch sender's balance"))?;

    println!(
        "deposit balance={} tx={signature}",
        sender_balance.amount
    );
    Ok(())
}

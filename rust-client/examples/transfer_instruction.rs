use anyhow::{anyhow, Result};
use rust_client_example::{create_test_recipient, env_config, setup_funded_sol_wallet};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::Transact;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{
    decrypt_transactions,
    instructions::{transact::ConfidentialTransfer, types::SppProofInputUtxo},
    AssetRegistry, SOL_MINT,
};

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
    let asset_registry = AssetRegistry::default();
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let sender_address = sender_keypair.shielded_address()?;
    let state_tree = Pubkey::new_from_array(client.tree().to_bytes());

    // Fund the sender's private balance and create the recipient's private wallet.
    let sender_wallet = setup_funded_sol_wallet(
        &client,
        &cfg.payer,
        &sender_keypair,
        1_000_000_000,
    )?;
    let recipient = create_test_recipient(&client, &cfg.payer, asset_registry.clone())?;
    let recipient_keypair = recipient.shielded_keypair;
    let recipient_address = recipient_keypair.shielded_address()?;

    // 1. Select UTXOs that make up the private balance for the transfer.
    let sender_utxo = sender_wallet
        .balance(SOL_MINT, None)
        .map_err(|error| anyhow!("read sender balance: {error:?}"))?
        .utxos
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("sender has no spendable SOL UTXO"))?;

    // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
    let input_utxos = vec![SppProofInputUtxo::new(
        sender_utxo,
        &sender_keypair,
    )];

    // 3. Build and sign the confidential transfer.
    // Signing encrypts the asset and amount and produces the proof inputs for the ZK prover.
    let mut transfer = ConfidentialTransfer::new(
        sender_address,
        input_utxos,
        payer,
    );
    transfer.send(
        &recipient_address,
        SOL_MINT,
        300_000_000,
    )?;
    let proof_inputs = transfer.sign(
        &sender_keypair,
        &asset_registry,
    )?;

    // 4. Fetch the zk proof to prove the sender can spend the balance without revealing asset and amount.
    let transfer_data = client.prove_transact(
        proof_inputs,
        Some(IndexerRpcConfig::wait()),
    )?;

    // 5. Construct the instruction.
    let transfer_instruction = Transact {
        payer,
        tree: state_tree,
        withdrawal: None,
        data: transfer_data,
    }
    .instruction();

    // 6. Send and confirm like any Solana transaction.
    let signature = client.create_and_send_transaction(
        &[transfer_instruction],
        payer,
        &[&cfg.payer],
    )?;
    client.confirm_private_transaction_sync(signature)?;

    // 7. Fetch and decrypt the recipient's balance.
    let recipient_tag = recipient_address.confidential_view_tag()?;
    let response = client.get_shielded_transactions_by_tags(
        vec![recipient_tag],
        None,
        None,
        Some(IndexerRpcConfig::wait()),
    )?;
    let recipient_balances = decrypt_transactions(
        &recipient_keypair,
        &response.transactions,
        &asset_registry,
    )
    .map_err(|error| anyhow!("decrypt recipient transactions: {error:?}"))?;
    let recipient_balance = recipient_balances
        .get_balance(SOL_MINT)
        .ok_or_else(|| anyhow!("failed to fetch recipient's balance"))?;

    println!(
        "transfer recipient_balance={} tx={signature}",
        recipient_balance.amount
    );
    Ok(())
}

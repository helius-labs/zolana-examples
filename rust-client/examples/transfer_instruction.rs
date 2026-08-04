use anyhow::{anyhow, Result};
use rust_client_example::{env_config, setup_funded_sol_wallet, DEFAULT_RECIPIENT};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::Transact;
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{
    instructions::{transact::ConfidentialTransfer, types::SppProofInputUtxo},
    AssetRegistry, SOL_MINT,
};
use zolana_wallet::resolve_registered_address;

fn main() -> Result<()> {
    // Load the funded fee payer and network settings, then connect.
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

    // Fund the sender's private balance and resolve the recipient's private
    // wallet by pubkey.
    let sender_wallet =
        setup_funded_sol_wallet(&client, &cfg.payer, &sender_keypair, 1_000_000_000)?;
    let recipient: Pubkey = DEFAULT_RECIPIENT.parse()?;
    // localnet: let recipient = rust_client_example::create_test_recipient(&client, &cfg.payer)?;
    let recipient = resolve_registered_address(&client, recipient)?;

    // Confidential SOL transfer to the recipient's private balance.
    // A confidential transfer reveals only sender and recipient, not the asset
    // or amount.

    // 1. Select private token accounts (UTXOs) that make up the private balance
    // for the transfer.
    let sender_utxo = sender_wallet
        .balance(SOL_MINT, None)
        // SPL: .balance(spl.mint, None)
        .map_err(|error| anyhow!("read sender balance: {error:?}"))?
        .utxos
        .into_iter()
        .max_by_key(|utxo| utxo.amount)
        .ok_or_else(|| anyhow!("sender has no spendable SOL UTXO"))?;

    // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
    let input_utxos = vec![SppProofInputUtxo::new(sender_utxo, &sender_keypair)];

    // 3. Build and sign the confidential transfer.
    // Signing encrypts the asset and amount and produces the proof inputs for the ZK prover.
    let mut transfer = ConfidentialTransfer::new(sender_address, input_utxos, payer);
    transfer.send(&recipient.address, SOL_MINT, 300_000_000)?;
    // SPL: transfer.send(&recipient.address, spl.mint, 300_000_000)?;
    let proof_inputs = transfer.sign(&sender_keypair, &asset_registry)?;

    // 4. Fetch the ZK proof to prove the sender can spend the balance without
    // revealing asset and amount.
    let transfer_data = client.prove_transact(
        cfg.tree_pubkey(),
        proof_inputs,
        Some(IndexerRpcConfig::wait()),
    )?;

    // 5. Build the instruction with the input and output state Merkle trees.
    // Private transfers move balances only between private token accounts, so
    // they require no public interface accounts.
    let transfer_instruction = Transact {
        payer,
        owner_signers: Vec::new(),
        input_tree: cfg.tree_pubkey(),
        output_tree: cfg.tree_pubkey(),
        interface_transfer_accounts: Vec::new(),
        data: transfer_data,
    }
    .instruction();

    // 6. Send and confirm like any Solana transaction.
    let signature =
        client.create_and_send_transaction(&[transfer_instruction], payer, &[&cfg.payer])?;
    client.confirm_private_transaction_sync(signature)?;

    // 7. Confirm the indexer lists the transfer under the recipient's public
    // viewing key.
    let mut delivered = false;
    for _ in 0..30 {
        let response = client.get_shielded_transactions_by_tags(
            vec![recipient.view_tag],
            None,
            None,
            Some(IndexerRpcConfig::wait()),
        )?;
        delivered = response
            .transactions
            .iter()
            .any(|tx| tx.tx_signature == signature);
        if delivered {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if !delivered {
        return Err(anyhow!("transfer not indexed for the recipient"));
    }

    println!("ok private transfer signature={signature} delivered=true");
    Ok(())
}

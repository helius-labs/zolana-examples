use anyhow::{anyhow, Result};
use rust_client_example::env_config;
use rust_client_example::setup_funded_sol_wallet;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::{
    Transact, TransactInterfaceTransferAccounts, TransactSolTransferAccounts,
};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{
    instructions::{
        transact::{ConfidentialTransfer, SettlementTarget},
        types::SppProofInputUtxo,
    },
    AssetRegistry, SOL_MINT,
};

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
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let sender_address = sender_keypair.shielded_address()?;
    let assets = AssetRegistry::default();

    // Fund the sender's private SOL balance.
    let sender_wallet =
        setup_funded_sol_wallet(&client, &cfg.payer, &sender_keypair, 1_000_000_000)?;
    let withdraw_amount = 300_000_000;

    // Withdraw SOL from the sender's private balance to their public balance.
    // A withdrawal reveals sender, recipient, asset, and amount.

    // 1. Select private token accounts (UTXOs) that make up the private balance
    // for the withdrawal.
    let sender_utxo = sender_wallet
        .balance(SOL_MINT, None)
        // SPL: .balance(spl.mint, None)
        .map_err(|error| anyhow!("read sender balance: {error:?}"))?
        .utxos
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("sender has no spendable SOL UTXO"))?;

    // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
    let input_utxos = vec![SppProofInputUtxo::new(sender_utxo, &sender_keypair)];

    // 3. Build and sign the private-to-public withdrawal.
    // Signing encrypts the asset and amount of the remaining private balance
    // and produces the proof inputs for the ZK prover.
    let mut withdrawal = ConfidentialTransfer::new(sender_address, input_utxos, payer);
    withdrawal.withdraw(
        SOL_MINT,
        withdraw_amount,
        SettlementTarget::Sol {
            user_sol_account: payer,
        },
    )?;
    // SPL: withdrawal.withdraw(
    // SPL:     spl.mint,
    // SPL:     withdraw_amount,
    // SPL:     SettlementTarget::Spl {
    // SPL:         user_spl_token: spl.user_token_account,
    // SPL:         spl_token_interface: spl.vault,
    // SPL:     },
    // SPL: )?;
    let proof_inputs = withdrawal.sign(&sender_keypair, &assets)?;

    // 4. Fetch the ZK proof to prove the sender can spend the balance.
    let withdrawal_data = client.prove_transact(
        cfg.tree_pubkey(),
        proof_inputs,
        Some(IndexerRpcConfig::wait()),
    )?;

    // 5. Build the instruction with the input and output state Merkle trees and
    // the public SOL account required for the withdrawal.
    let withdrawal_instruction = Transact {
        payer,
        owner_signers: Vec::new(),
        input_tree: cfg.tree_pubkey(),
        output_tree: cfg.tree_pubkey(),
        interface_transfer_accounts: vec![TransactInterfaceTransferAccounts::Sol(
            TransactSolTransferAccounts { recipient: payer },
        )],
        // SPL: interface_transfer_accounts: vec![
        // SPL:     TransactInterfaceTransferAccounts::SplWithdrawal(
        // SPL:         zolana_interface::instruction::TransactSplWithdrawalAccounts {
        // SPL:             mint: spl.mint,
        // SPL:             vault: spl.vault,
        // SPL:             user_token_account: spl.user_token_account,
        // SPL:             token_program: spl.token_program,
        // SPL:         },
        // SPL:     ),
        // SPL: ],
        data: withdrawal_data,
    }
    .instruction();

    // 6. Send and confirm like any Solana transaction.
    let signature =
        client.create_and_send_transaction(&[withdrawal_instruction], payer, &[&cfg.payer])?;
    client.confirm_private_transaction_sync(signature)?;

    // 7. Report the public SOL withdrawal.
    println!(
        "ok withdrawal amount={} recipient={} tx={signature}",
        withdraw_amount, payer,
    );
    Ok(())
}

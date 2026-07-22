use anyhow::{anyhow, Result};
use rust_client_example::{env_config, setup_funded_wallet};
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::{
    instruction::{Transact, TransactSplWithdrawal, TransactWithdrawal},
    pda,
};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::instructions::{
    transact::{ConfidentialTransfer, WithdrawalTarget},
    types::SppProofInputUtxo,
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
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let sender_address = sender_keypair.shielded_address()?;

    // Fund the sender's private SPL balance.
    let funded = setup_funded_wallet(&client, &cfg.payer, &sender_keypair, 10_000)?;
    let mint = funded.asset.mint;
    let vault = pda::spl_asset_vault(&mint);
    let withdraw_amount = 3_000;

    // 1. Select UTXOs that make up the private balance for the withdrawal.
    let sender_utxo = funded
        .wallet
        .balance(mint, None)
        .map_err(|error| anyhow!("read sender balance: {error:?}"))?
        .utxos
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("sender has no spendable SPL UTXO"))?;

    // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
    let input_utxos = vec![SppProofInputUtxo::new(sender_utxo, &sender_keypair)];

    // 3. Build and sign the confidential withdrawal.
    // Signing encrypts the private change and produces the ZK prover inputs.
    let mut withdrawal = ConfidentialTransfer::new(sender_address, input_utxos, payer);
    withdrawal.withdraw(
        mint,
        withdraw_amount,
        WithdrawalTarget::Spl {
            user_spl_token: funded.asset.user_token,
            spl_token_interface: vault,
        },
    )?;
    let proof_inputs = withdrawal.sign(&sender_keypair, &funded.registry)?;

    // 4. Fetch the ZK proof to prove the sender can spend the balance.
    let withdrawal_data = client.prove_transact(proof_inputs, Some(IndexerRpcConfig::wait()))?;

    // 5. Combine the proof and withdrawal accounts in a single instruction.
    let withdrawal_instruction = Transact {
        payer,
        tree: client.tree(),
        withdrawal: Some(TransactWithdrawal::Spl(TransactSplWithdrawal {
            cpi_authority: Some(pda::shielded_pool_cpi_authority()),
            spl_token_interface: vault,
            recipient: payer,
            user_token_account: funded.asset.user_token,
            token_program: pda::spl_token_program_id(),
        })),
        data: withdrawal_data,
    }
    .instruction();

    // 6. Send and confirm like any Solana transaction.
    let signature =
        client.create_and_send_transaction(&[withdrawal_instruction], payer, &[&cfg.payer])?;
    client.confirm_private_transaction_sync(signature)?;

    // 7. Report the public SPL withdrawal.
    println!(
        "withdraw amount={} user_token={} tx={signature}",
        withdraw_amount, funded.asset.user_token,
    );
    Ok(())
}

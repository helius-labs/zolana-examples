use anyhow::{anyhow, Result};
use rust_client_example::{
    client, env_config, shielded_keypair, setup_funded_sol_wallet, tree_pubkey,
};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc};
use zolana_interface::instruction::{Transact, TransactSolWithdrawal, TransactWithdrawal};
use zolana_transaction::{
    instructions::{
        transact::{ConfidentialTransfer, WithdrawalTarget},
        types::SppProofInputUtxo,
    },
    AssetRegistry, SOL_MINT,
};

const FUND_AMOUNT: u64 = 1_000_000_000;
const WITHDRAW_AMOUNT: u64 = 300_000_000;

// Withdraw a private balance back to a public account, building the transact
// instruction by hand.
fn main() -> Result<()> {
    let cfg = env_config()?;
    let client = client(&cfg);
    let assets = AssetRegistry::default();
    let alice = shielded_keypair(&cfg.payer)?;
    let alice_address = alice.shielded_address()?;
    let tree = tree_pubkey(&client);

    // Setup: fund Alice's private balance.
    let wallet = setup_funded_sol_wallet(&client, &cfg.payer, &alice, FUND_AMOUNT)?;

    // Take the balance Alice holds as the spend input.
    let utxo = wallet
        .balances(false)
        .map_err(|e| anyhow!("read alice balance: {e:?}"))?
        .into_iter()
        .find(|b| b.mint == SOL_MINT)
        .expect("alice has no sol balance")
        .utxos[0]
        .clone();

    // Send WITHDRAW_AMOUNT back to Alice's own Solana account and authorize it.
    let mut withdrawal =
        ConfidentialTransfer::new(alice_address, vec![SppProofInputUtxo::new(utxo, &alice)], cfg.payer.pubkey());
    withdrawal.withdraw(
        SOL_MINT,
        WITHDRAW_AMOUNT,
        WithdrawalTarget::Sol {
            user_sol_account: Address::new_from_array(cfg.payer.pubkey().to_bytes()),
        },
    )?;
    let proof_inputs = withdrawal.sign(&alice, &assets)?;

    // Build the on-chain data, which includes the proof that Alice owns and can
    // spend the balance, then send it with the withdrawal accounts.
    let data = client.prove_transact(proof_inputs, Some(IndexerRpcConfig::wait()))?;
    let withdraw_ix = Transact {
        payer: cfg.payer.pubkey(),
        tree,
        withdrawal: Some(TransactWithdrawal::Sol(TransactSolWithdrawal {
            recipient: cfg.payer.pubkey(),
        })),
        data,
    }
    .instruction();
    let signature =
        client.create_and_send_transaction(&[withdraw_ix], cfg.payer.pubkey(), &[&cfg.payer])?;
    client.confirm_private_transaction_sync(signature)?;

    // Confirm the withdrawn amount landed in Alice's Solana balance.
    let solana_balance = client.get_balance(cfg.payer.pubkey())?;
    println!("withdraw solana_balance={solana_balance} tx={signature}");
    Ok(())
}

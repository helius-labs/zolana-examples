use anyhow::{anyhow, Result};
use rust_client_example::{
    client, create_test_recipient, env_config, shielded_keypair, setup_funded_sol_wallet,
    tree_pubkey,
};
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc};
use zolana_interface::instruction::Transact;
use zolana_transaction::{
    decrypt_transactions,
    instructions::{transact::ConfidentialTransfer, types::SppProofInputUtxo},
    AssetRegistry, SOL_MINT,
};

const FUND_AMOUNT: u64 = 1_000_000_000;
const TRANSFER_AMOUNT: u64 = 300_000_000;

// Send a private transfer to Bob, building the transact instruction by hand.
fn main() -> Result<()> {
    let cfg = env_config()?;
    let client = client(&cfg);
    let assets = AssetRegistry::default();
    let alice = shielded_keypair(&cfg.payer)?;
    let alice_address = alice.shielded_address()?;
    let tree = tree_pubkey(&client);

    // Setup: fund Alice's private balance and create a registered recipient.
    let wallet = setup_funded_sol_wallet(&client, &cfg.payer, &alice, FUND_AMOUNT)?;
    let bob = create_test_recipient(&client, &cfg.payer, AssetRegistry::default())?;
    let bob_address = bob.shielded_keypair.shielded_address()?;

    // Take the balance Alice holds as the spend input.
    let utxo = wallet
        .balances(false)
        .map_err(|e| anyhow!("read alice balance: {e:?}"))?
        .into_iter()
        .find(|b| b.mint == SOL_MINT)
        .expect("alice has no sol balance")
        .utxos[0]
        .clone();

    // Route TRANSFER_AMOUNT to Bob and authorize the spend.
    let mut transfer =
        ConfidentialTransfer::new(alice_address, vec![SppProofInputUtxo::new(utxo, &alice)], cfg.payer.pubkey());
    transfer.send(&bob_address, SOL_MINT, TRANSFER_AMOUNT)?;
    let proof_inputs = transfer.sign(&alice, &assets)?;

    // Build the on-chain data, which includes the proof that Alice owns and can
    // spend the balance, then send it.
    let data = client.prove_transact(proof_inputs, Some(IndexerRpcConfig::wait()))?;
    let transfer_ix = Transact {
        payer: cfg.payer.pubkey(),
        tree,
        withdrawal: None,
        data,
    }
    .instruction();
    let signature =
        client.create_and_send_transaction(&[transfer_ix], cfg.payer.pubkey(), &[&cfg.payer])?;
    client.confirm_private_transaction_sync(signature)?;

    // Read Bob's balance to confirm the transfer landed.
    let response = client.get_shielded_transactions_by_tags(
        vec![bob_address.confidential_view_tag()?],
        None,
        None,
        Some(IndexerRpcConfig::wait()),
    )?;
    let bob_balances = decrypt_transactions(&bob.shielded_keypair, &response.transactions, &assets)
        .map_err(|e| anyhow!("decrypt bob transactions: {e:?}"))?;
    let bob_balance = bob_balances
        .get_balance(SOL_MINT)
        .expect("failed to fetch bob's balance");

    println!("transfer bob_balance={} tx={signature}", bob_balance.amount);
    Ok(())
}

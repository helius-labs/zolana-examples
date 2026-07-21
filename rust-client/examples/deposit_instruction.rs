use anyhow::{anyhow, Result};
use rust_client_example::{client, env_config, shielded_keypair, tree_pubkey};
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc};
use zolana_interface::instruction::Deposit;
use zolana_keypair::random_blinding;
use zolana_transaction::{decrypt_transactions, AssetRegistry, SOL_MINT};

const DEPOSIT_AMOUNT: u64 = 1_000_000_000;

// Deposit public SOL into a private balance, building the deposit instruction
// by hand.
fn main() -> Result<()> {
    let cfg = env_config()?;
    let client = client(&cfg);
    let assets = AssetRegistry::default();
    let alice = shielded_keypair(&cfg.payer)?;
    let alice_address = alice.shielded_address()?;
    let tree = tree_pubkey(&client);

    // Move public SOL into Alice's private balance.
    let deposit_ix = Deposit {
        tree,
        depositor: cfg.payer.pubkey(),
        spl: None,
        view_tag: alice_address.confidential_view_tag()?,
        owner: alice_address.owner_hash()?,
        blinding: random_blinding(),
        amount: DEPOSIT_AMOUNT,
        utxo_data: None,
        memo: None,
    }
    .instruction();
    let signature =
        client.create_and_send_transaction(&[deposit_ix], cfg.payer.pubkey(), &[&cfg.payer])?;

    // Read the balance back from the indexer. A deposit is sent in the clear, so
    // there is nothing to decrypt.
    let response = client.get_shielded_transactions_by_tags(
        vec![alice_address.confidential_view_tag()?],
        None,
        Some(50),
        Some(IndexerRpcConfig::wait()),
    )?;
    let balances = decrypt_transactions(&alice, &response.transactions, &assets)
        .map_err(|e| anyhow!("decrypt alice transactions: {e:?}"))?;
    let balance = balances
        .get_balance(SOL_MINT)
        .expect("failed to fetch alice's balance");

    println!("deposit balance={} tx={signature}", balance.amount);
    Ok(())
}

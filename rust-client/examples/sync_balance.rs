use anyhow::{anyhow, Result};
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{decrypt_transactions, AssetRegistry};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        prover_url,
        tree,
    );

    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;
    let assets = AssetRegistry::default();

    // Fetch transaction outputs from the indexer.
    let response = client.get_shielded_transactions_by_tags(
        vec![sender.shielded_address()?.confidential_view_tag()?],
        None,
        Some(50),
        None,
    )?;

    // Decrypt locally to read the private balances.
    let balances = decrypt_transactions(&sender, &response.transactions, &assets)
        .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;

    for b in &balances.assets {
        println!(
            "ok solana_address={} mint={} amount={} utxos={}",
            sender.pubkey(),
            b.mint,
            b.amount,
            b.utxos.len(),
        );
    }
    Ok(())
}

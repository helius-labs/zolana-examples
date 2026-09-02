use anyhow::{anyhow, Result};
use rust_client_example::{connect, ConnectContext};
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_transaction::{decrypt_transactions, AssetRegistry};

fn main() -> Result<()> {
    let ConnectContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
        wallet,
    } = connect()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        prover_url,
        tree,
    );

    let assets = AssetRegistry::default();
    let address = wallet.shielded_address()?;

    // Fetch transaction outputs from the indexer.
    let response = client.get_shielded_transactions_by_tags(
        vec![address.confidential_view_tag()?],
        None,
        Some(50),
        None,
    )?;

    // Decrypt locally to read the private balances.
    let balances = decrypt_transactions(&wallet, &response.transactions, &assets)
        .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;

    let solana_address = address.solana_address()?;
    for b in &balances.assets {
        println!(
            "ok solana_address={solana_address} mint={} amount={} utxos={}",
            b.mint,
            b.amount,
            b.utxos.len(),
        );
    }
    Ok(())
}

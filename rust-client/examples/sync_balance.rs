use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_signer::Signer;
use zolana_client::{SolanaRpc, SpendableUtxos, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::AssetRegistry;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;
    let assets = AssetRegistry::default();

    // Fetch the transactions tagged for this wallet and decrypt its spendable UTXOs.
    let spendable = SpendableUtxos::new(&sender, &assets).fetch(&client)?;
    anyhow::ensure!(
        spendable.unknown_asset_ids.is_empty(),
        "could not resolve SPL asset registrations"
    );

    for b in &spendable.balances.assets {
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

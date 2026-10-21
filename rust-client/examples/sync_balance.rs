use anyhow::Result;
use rust_client_example::{asset_registry, cli_keypair, setup, SetupContext};
use solana_keypair::Keypair;
use solana_signer::Signer;
use zolana_client::{SolanaRpc, UtxoScan, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::Balances;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    run(&client, &cli_keypair()?).map(|_| ())
}

pub fn run(client: &ZolanaClient<SolanaRpc>, payer: &Keypair) -> Result<Balances> {
    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(payer)?;
    let assets = asset_registry(client)?;

    // Fetch the transactions tagged for this wallet and decrypt its spendable UTXOs.
    let spendable = UtxoScan::new(&sender, &assets).fetch(client)?.spendable;
    anyhow::ensure!(
        spendable.unknown_asset_ids.is_empty() && spendable.unknown_mints.is_empty(),
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
    Ok(spendable.balances)
}

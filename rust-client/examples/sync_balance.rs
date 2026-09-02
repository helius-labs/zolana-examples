use anyhow::{anyhow, Result};
use rust_client_example::{connect, ConnectContext};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_transaction::{decrypt_transactions, AssetRegistry, SOL_MINT};

// House mock-USDC on the zolana demo.
const HOUSE_USDC_MINT: &str = "CjHHSnWtR17GVhFmvAtBvcrvPPDU3XovsnSf3RKEySCc";
const HOUSE_USDC_ASSET_ID: u64 = 6;

fn main() -> Result<()> {
    let ConnectContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
        wallet,
    } = connect()?;

    // Load the funded fee payer and devnet settings, then connect.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        prover_url,
        tree,
    );

    // Mints that are registered with Solana Rings for privacy.
    let mut assets = AssetRegistry::default();
    let mint = HOUSE_USDC_MINT
        .parse::<Address>()
        .map_err(|e| anyhow!("parse house USDC mint: {e}"))?;
    assets.insert(HOUSE_USDC_ASSET_ID, mint)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender_solana_keypair = wallet.to_solana_keypair()?;
    let sender_tag = wallet.shielded_address()?.confidential_view_tag()?;

    // Fetch transaction outputs from the indexer.
    // The indexer returns encrypted outputs by view tag, the sender's public key in Confidential Rings.
    let response =
        client.get_shielded_transactions_by_tags(vec![sender_tag], None, Some(50), None)?;

    // The sender decrypts the transaction outputs locally to update the private balance.
    let balances = decrypt_transactions(&wallet, &response.transactions, &assets)
        .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;

    let sol = balances.get_balance(SOL_MINT);
    println!(
        "ok solana_address={} sol={} utxos={}",
        sender_solana_keypair.pubkey(),
        sol.map(|b| b.amount).unwrap_or(0),
        sol.map(|b| b.utxos.len()).unwrap_or(0),
    );
    let spl = balances.get_balance(mint);
    println!(
        "ok mint={mint} spl={} utxos={}",
        spl.map(|b| b.amount).unwrap_or(0),
        spl.map(|b| b.utxos.len()).unwrap_or(0),
    );
    Ok(())
}

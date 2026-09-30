use anyhow::Result;
use rust_client_example::{cli_keypair, setup, SetupContext};
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{AssetRegistry, ShieldedTransaction, Wallet, DEFAULT_TAG_WINDOW};

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        ..
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // The Solana signer and the private wallet are derived from the same
    // Ed25519 seed.
    let owner = ShieldedKeypair::from_keypair(&cli_keypair()?)?;

    // Fetch every confidential transaction tagged for this wallet.
    let owner_tag = owner
        .shielded_address()?
        .signing_pubkey
        .confidential_view_tag()?;
    let mut transactions: Vec<ShieldedTransaction> = Vec::new();
    let mut cursor = None;
    loop {
        let page =
            client.get_shielded_transactions_by_tags(vec![owner_tag], cursor, Some(50), None)?;
        transactions.extend(page.transactions);
        let Some(next) = page.next_cursor else { break };
        cursor = Some(next);
    }

    // Decrypt them into a wallet, which also records each one as history.
    let mut wallet = Wallet::new(owner.shielded_address()?, AssetRegistry::default())?;
    wallet.sync(&owner, &transactions, 0, DEFAULT_TAG_WINDOW)?;
    for tx in wallet.private_transactions() {
        println!(
            "ok kind={:?} direction={:?} mint={} amount={} tx={}",
            tx.kind, tx.direction, tx.asset, tx.amount, tx.id.signature,
        );
    }
    Ok(())
}

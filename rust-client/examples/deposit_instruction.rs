use anyhow::{anyhow, Result};
use rust_client_example::env_config;
use solana_signer::Signer;
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::{AssetDeposit, Deposit, DepositAsset};
use zolana_keypair::{random_blinding, ShieldedKeypair};
use zolana_transaction::{decrypt_transactions, AssetRegistry, SOL_MINT};

fn main() -> Result<()> {
    // Load the funded fee payer and network settings, then connect.
    let cfg = env_config()?;
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(cfg.rpc_url.clone()),
        &cfg.indexer_url,
        cfg.prover_url.clone(),
        cfg.tree,
    );
    let payer = cfg.payer.pubkey();
    let sender_keypair = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let sender_address = sender_keypair.shielded_address()?;
    let assets = AssetRegistry::default();

    // Deposit SOL into the sender's private balance.
    // A deposit from a public balance reveals sender, recipient, asset, and
    // amount. Alternatively, you can onramp fiat directly to a private balance.

    // 1. Move public SOL into the sender's private balance.
    let sender_tag = sender_address.confidential_view_tag()?;
    let deposit_amount = 1_000_000_000;
    let deposit_instruction = Deposit {
        tree: cfg.tree_pubkey(),
        depositor: payer,
        deposits: vec![AssetDeposit {
            asset: DepositAsset::Sol,
            // SPL: asset: DepositAsset::Spl(
            // SPL:     zolana_interface::instruction::DepositSplAccounts {
            // SPL:         mint: spl.mint,
            // SPL:         user_token: spl.user_token_account,
            // SPL:         token_program: spl.token_program,
            // SPL:     },
            // SPL: ),
            view_tag: sender_tag,
            owner: sender_address.owner_hash()?,
            blinding: random_blinding(),
            amount: deposit_amount,
            utxo_data: None,
            memo: None,
        }],
    }
    .instruction()?;

    // 2. Send like any Solana transaction.
    let signature =
        client.create_and_send_transaction(&[deposit_instruction], payer, &[&cfg.payer])?;

    // 3. Fetch transaction outputs from the indexer. The indexer returns
    // encrypted outputs by view tag: the sender's public viewing key in
    // Confidential Rings.
    let response =
        client.get_shielded_transactions_by_tags(vec![sender_tag], None, Some(50), None)?;

    // 4. The sender decrypts the transaction outputs locally to update the
    // private balance.
    let sender_balances = decrypt_transactions(&sender_keypair, &response.transactions, &assets)
        .map_err(|error| anyhow!("decrypt sender transactions: {error:?}"))?;
    let sender_balance = sender_balances
        .get_balance(SOL_MINT)
        .ok_or_else(|| anyhow!("failed to fetch sender's balance"))?;

    println!(
        "ok deposit private_balance={} tx={signature}",
        sender_balance.amount
    );
    Ok(())
}

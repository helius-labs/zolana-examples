use anyhow::{anyhow, Result};
use rust_client_example::{create_private_wallet, deposit_sol, deposit_spl, register_asset};
use solana_address::Address;
use solana_keypair::read_keypair_file;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    get_private_token_balances, sync_wallet, ProverClient, SolanaRpc, Submit,
    Transaction as ClientTransaction, ZolanaIndexer,
};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_transaction::{Utxo, SOL_MINT};

fn main() -> Result<()> {
    // Connect to the devnet deployment.
    let indexer = ZolanaIndexer::new("http://202.8.10.77:8784/");
    let rpc_url = format!(
        "https://devnet.helius-rpc.com/?api-key={}",
        std::env::var("API_KEY").expect("set API_KEY"),
    );
    let mut rpc = SolanaRpc::new(rpc_url).with_indexer(indexer.clone());
    let prover = ProverClient::new("http://202.8.10.77:3011".to_string());
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| format!("{}/.config/solana/id.json", std::env::var("HOME").unwrap_or_default()));
    let payer = read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree: Pubkey = std::env::var("ZOLANA_TREE").expect("set ZOLANA_TREE").parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Send an SPL value. For a SOL transfer, skip register_asset, build the wallets
    // from `AssetRegistry::default()`, and use `SOL_MINT` as the asset below (SOL
    // then covers both the value and the fee).
    let (asset, registry) = register_asset(&mut rpc, &payer)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let (sender_keypair, _sender_funding, mut sender_wallet) =
        create_private_wallet(&mut rpc, &payer, registry.clone())?;
    // Transfers privately to a recipient with a private wallet, otherwise falls back to a private-to-public withdrawal
    let (recipient_keypair, _recipient_funding, mut recipient_wallet) =
        create_private_wallet(&mut rpc, &payer, registry)?;

    // Deposit an SPL asset to send and SOL for the transaction fee
    deposit_spl(
        &rpc,
        &payer,
        tree,
        &indexer,
        &sender_keypair,
        &mut sender_wallet,
        &asset,
        10_000,
    )?;
    deposit_sol(
        &rpc,
        &payer,
        tree,
        &indexer,
        &sender_keypair,
        &mut sender_wallet,
        5_000_000,
    )?;

    // Sync the wallet to see the current balance before spending it
    sync_wallet(&mut sender_wallet, &indexer)?;

    // Select the SPL asset to send and SOL for the transaction fee
    let mut inputs: Vec<Utxo> = Vec::new();
    for want in [asset_address, SOL_MINT] {
        let utxo = sender_wallet
            .utxos
            .iter()
            .find(|w| !w.spent && w.utxo.asset == want)
            .map(|w| w.utxo.clone())
            .expect("deposited note present");
        inputs.push(utxo);
    }

    // Build and sign the private transfer
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    let mut tx = ClientTransaction::from_wallet(&sender_wallet, &inputs, payer_address)?;
    tx.send(&recipient_keypair.shielded_address()?, asset_address, 4_000)?;
    let signed = tx.sign(&sender_keypair, &sender_wallet.registry)?;

    let submit = Submit {
        signed,
        withdrawal: None,
        cu_limit: None,
    };
    let signature = submit.execute(&rpc, &prover, &payer, tree)?;

    // Sync the recipient's private balance.
    sync_wallet(&mut recipient_wallet, &indexer)?;
    let balance = get_private_token_balances(&recipient_wallet)?;

    println!("ok private transfer signature={signature} recipient_private_balance={balance:?}");
    Ok(())
}

use anyhow::{anyhow, Result};
use rust_client_example::{create_private_wallet, deposit_sol, deposit_spl, register_asset};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, create_withdrawal_sync, sync_wallet, CreateWithdrawal,
    ProverClient, SolanaRpc, Submit, ZolanaIndexer,
};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;

fn main() -> Result<()> {
    // Load .env if present.
    dotenvy::dotenv().ok();

    // Connect to devnet.
    let indexer = ZolanaIndexer::new("http://202.8.10.77:8784/");
    let rpc_url = format!(
        "https://devnet.helius-rpc.com/?api-key={}",
        std::env::var("API_KEY").expect("set API_KEY"),
    );
    let mut rpc = SolanaRpc::new(rpc_url);
    let prover = ProverClient::new("http://202.8.10.77:3011".to_string());
    let payer_path = std::env::var("ZOLANA_PAYER_KEYPAIR").unwrap_or_else(|_| {
        format!(
            "{}/.config/solana/id.json",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let payer =
        read_keypair_file(&payer_path).map_err(|e| anyhow!("load payer {payer_path}: {e}"))?;
    let tree: Pubkey = std::env::var("ZOLANA_TREE")
        .expect("set ZOLANA_TREE")
        .parse()?;
    rpc.assert_executable(&Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID))?;

    // Create a test mint with an interface PDA for private balances and transactions,
    // then create a private wallet.
    let (asset, registry) = register_asset(&mut rpc, &payer)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let (keypair, mut wallet) = create_private_wallet(&mut rpc, &payer, registry)?;

    // Deposit an SPL asset to withdraw and SOL for the transaction fee
    deposit_spl(
        &rpc,
        &payer,
        tree,
        &indexer,
        &keypair,
        &mut wallet,
        &asset,
        10_000,
    )?;
    deposit_sol(
        &rpc,
        &payer,
        tree,
        &indexer,
        &keypair,
        &mut wallet,
        5_000_000,
    )?;

    // Recipient for withdrawal can be owner or third party.
    let recipient = Keypair::new();
    let (_ata_sig, ata) =
        create_associated_token_account(&rpc, &payer, &recipient.pubkey(), &asset.mint)?;

    // Sync the wallet to see the current balance before spending it
    sync_wallet(&mut wallet, &indexer)?;

    // Build and sign the private-to-public withdrawal.
    let owner_address = Address::new_from_array(payer.pubkey().to_bytes());
    let withdrawal = create_withdrawal_sync(CreateWithdrawal {
        wallet: &wallet,
        authority: &keypair,
        owner_pubkey: Pubkey::default(),
        payer: owner_address,
        recipient: recipient.pubkey(),
        asset: asset_address,
        amount: 4_000,
    })?;

    // Prove and submit the withdrawal. The proof shows the sender owns the balance
    // being spent and has not already spent it.
    let signature = Submit {
        indexer: &indexer,
        rpc: &rpc,
        prover: &prover,
        payer: &payer,
        tree,
        cu_limit: None,
    }
    .execute(
        withdrawal.signed,
        Some(withdrawal.withdrawal),
        withdrawal.wait_tag,
    )?;

    // Sync the private balance.
    sync_wallet(&mut wallet, &indexer)?;

    println!("ok withdrawal signature={signature} recipient_token_account={ata}");
    Ok(())
}

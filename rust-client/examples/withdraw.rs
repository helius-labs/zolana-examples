use anyhow::{anyhow, Result};
use rust_client_example::{create_private_wallet, deposit_sol, deposit_spl, fund_key, register_asset};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, sync_wallet, ProverClient, SolanaRpc, Submit,
    Transaction as ClientTransaction, WithdrawalTarget, ZolanaIndexer,
};
use zolana_interface::{
    instruction::{TransactSplWithdrawal, TransactWithdrawal},
    pda, SHIELDED_POOL_PROGRAM_ID, SPL_TOKEN_PROGRAM_ID,
};
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

    // Withdraw an SPL value. For a SOL withdrawal, skip register_asset, build the
    // wallet from `AssetRegistry::default()`, use `SOL_MINT` as the asset, and
    // target `WithdrawalTarget::Sol` / `TransactWithdrawal::Sol` (a plain
    // recipient, no ATA or vault).
    let (asset, registry) = register_asset(&mut rpc, &payer)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let (keypair, _funding, mut wallet) = create_private_wallet(&mut rpc, &payer, registry)?;

    // Deposit an SPL asset to withdraw and SOL for the transaction fee
    deposit_spl(&rpc, &payer, tree, &indexer, &keypair, &mut wallet, &asset, 10_000)?;
    deposit_sol(&rpc, &payer, tree, &indexer, &keypair, &mut wallet, 5_000_000)?;

    // A withdrawal exits to a public account. Recipient's token account is created idempotently
    let recipient = Keypair::new();
    fund_key(&mut rpc, &payer, &recipient.pubkey(), 1_000_000)?;
    let (_ata_sig, ata) =
        create_associated_token_account(&rpc, &payer, &recipient.pubkey(), &asset.mint)?;
    let interface_pda = pda::spl_asset_vault(&asset.mint);

    // Sync the wallet to see the current balance before spending it
    sync_wallet(&mut wallet, &indexer)?;

    // Select the SPL asset to withdraw and SOL for the transaction fee
    let mut inputs: Vec<Utxo> = Vec::new();
    for want in [asset_address, SOL_MINT] {
        let utxo = wallet
            .utxos
            .iter()
            .find(|w| !w.spent && w.utxo.asset == want)
            .map(|w| w.utxo.clone())
            .expect("deposited note present");
        inputs.push(utxo);
    }

    // Build and sign the withdrawal
    let payer_address = Address::new_from_array(payer.pubkey().to_bytes());
    let mut tx = ClientTransaction::from_wallet(&wallet, &inputs, payer_address)?;
    tx.withdraw(
        asset_address,
        4_000,
        WithdrawalTarget::Spl {
            user_spl_token: Address::new_from_array(ata.to_bytes()),
            spl_token_interface: Address::new_from_array(interface_pda.to_bytes()),
        },
    )?;
    let signed = tx.sign(&keypair, &wallet.registry)?;

    // Withdraw private balance to recipient's public balance
    let withdrawal = TransactWithdrawal::Spl(TransactSplWithdrawal {
        cpi_authority: Some(pda::shielded_pool_cpi_authority()),
        spl_token_interface: interface_pda,
        recipient: recipient.pubkey(),
        user_token_account: ata,
        token_program: Pubkey::new_from_array(SPL_TOKEN_PROGRAM_ID),
    });

    let submit = Submit {
        signed,
        withdrawal: Some(withdrawal),
        cu_limit: None,
    };
    let signature = submit.execute(&rpc, &prover, &payer, tree)?;

    // Sync the private balance.
    sync_wallet(&mut wallet, &indexer)?;

    println!("ok withdrawal signature={signature} recipient_token_account={ata}");
    Ok(())
}

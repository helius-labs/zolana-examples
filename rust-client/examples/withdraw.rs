use anyhow::Result;
use rust_client_example::{
    deposit_sol, deposit_spl, ensure_spl_asset, setup_localnet, setup_private_wallet,
};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use zolana_client::{
    create_associated_token_account, sync_wallet, Submit, Transaction as ClientTransaction,
    WithdrawalTarget,
};
use zolana_interface::{
    instruction::{TransactSplWithdrawal, TransactWithdrawal},
    pda, SPL_TOKEN_PROGRAM_ID,
};
use zolana_test_utils::test_validator_asserts::wait_for_indexed_transaction;
use zolana_transaction::{Utxo, SOL_MINT};

fn main() -> Result<()> {
    let (mut client, mut localnet) = setup_localnet()?;
    let asset = ensure_spl_asset(&mut client, &mut localnet)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let (keypair, _funding, mut wallet) = setup_private_wallet(&mut client, &localnet)?;

    // Deposit an SPL asset to withdraw and SOL for the transaction fee
    deposit_spl(
        &mut client,
        &keypair,
        &mut wallet,
        &asset,
        10_000,
    )?;
    deposit_sol(&mut client, &keypair, &mut wallet, 5_000_000)?;

    // A withdrawal exits to a public account. Recipient's token account is created idempotently
    let recipient = Keypair::new();
    client.rpc.airdrop(&recipient.pubkey(), 1_000_000)?;
    let (_ata_sig, ata) = create_associated_token_account(
        &client.rpc,
        &client.payer,
        &recipient.pubkey(),
        &asset.mint,
    )?;
    let vault = pda::spl_asset_vault(&asset.mint);

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
    let payer = Address::new_from_array(client.payer.pubkey().to_bytes());
    let mut tx = ClientTransaction::from_wallet(&wallet, &inputs, payer)?;
    tx.withdraw(
        asset_address,
        4_000,
        WithdrawalTarget::Spl {
            user_spl_token: Address::new_from_array(ata.to_bytes()),
            spl_token_interface: Address::new_from_array(vault.to_bytes()),
        },
    )?;
    let signed = tx.sign(&keypair, &wallet.registry)?;
    let wait_tag = keypair.signing_pubkey().confidential_view_tag()?;

    // Withdraw private balance to recipient's public balance
    let withdrawal = TransactWithdrawal::Spl(TransactSplWithdrawal {
        cpi_authority: Some(pda::shielded_pool_cpi_authority()),
        vault,
        recipient: recipient.pubkey(),
        user_token_account: ata,
        token_program: solana_pubkey::Pubkey::new_from_array(SPL_TOKEN_PROGRAM_ID),
    });

    let submit = Submit {
        signed,
        withdrawal: Some(withdrawal),
        cu_limit: None,
    };
    let payer_keypair = client.payer.insecure_clone();
    let signature = submit.execute(&client.rpc, &client.prover, &payer_keypair, client.tree)?;
    // Let indexer catch up for sync of private balances
    wait_for_indexed_transaction(&client.indexer, wait_tag, signature);

    sync_wallet(&mut wallet, &client.indexer)?;

    println!("ok withdrawal signature={signature} recipient_token_account={ata}");
    Ok(())
}

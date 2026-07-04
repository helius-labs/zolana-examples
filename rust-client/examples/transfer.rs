use anyhow::Result;
use rust_client_example::{
    deposit_sol, deposit_spl, register_asset, setup, setup_private_wallet,
};
use solana_address::Address;
use solana_signer::Signer;
use zolana_client::{
    get_private_token_balances, sync_wallet, Submit, Transaction as ClientTransaction,
};
use zolana_transaction::{Utxo, SOL_MINT};

fn main() -> Result<()> {
    let (mut client, mut localnet) = setup()?;
    // Send an SPL value. For a SOL transfer, skip register_asset and use
    // `SOL_MINT` as the asset below (SOL then covers both the value and the fee).
    let asset = register_asset(&mut client, &mut localnet)?;
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let (sender_keypair, _sender_funding, mut sender_wallet) =
        setup_private_wallet(&mut client, &localnet)?;
    // Transfers privately to a recipient with a private wallet, otherwise falls back to a private-to-public withdrawal
    let (recipient_keypair, _recipient_funding, mut recipient_wallet) =
        setup_private_wallet(&mut client, &localnet)?;

    // Deposit an SPL asset to send and SOL for the transaction fee
    deposit_spl(
        &mut client,
        &sender_keypair,
        &mut sender_wallet,
        &asset,
        10_000,
    )?;
    deposit_sol(&mut client, &sender_keypair, &mut sender_wallet, 5_000_000)?;

    // Sync the wallet to see the current balance before spending it
    sync_wallet(&mut sender_wallet, &client.indexer)?;

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
    let payer = Address::new_from_array(client.payer.pubkey().to_bytes());
    let mut tx = ClientTransaction::from_wallet(&sender_wallet, &inputs, payer)?;
    tx.send(&recipient_keypair.shielded_address()?, asset_address, 4_000)?;
    let signed = tx.sign(&sender_keypair, &sender_wallet.registry)?;

    let submit = Submit {
        signed,
        withdrawal: None,
        cu_limit: None,
    };
    let payer_keypair = client.payer.insecure_clone();
    let signature = submit.execute(&client.rpc, &client.prover, &payer_keypair, client.tree)?;

    // Sync the recipient's private balance.
    sync_wallet(&mut recipient_wallet, &client.indexer)?;
    let balance = get_private_token_balances(&recipient_wallet)?;

    println!("ok private transfer signature={signature} recipient_private_balance={balance:?}");
    Ok(())
}

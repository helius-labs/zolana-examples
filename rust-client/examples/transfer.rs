use anyhow::{anyhow, Result};
use rust_client_example::{create_test_recipient, env_config, setup_funded_wallet};
use solana_address::Address;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zolana_client::{
    create_transfer_sync, get_private_token_balances, sync_wallet, CreateTransfer, Submit,
    ZolanaClient,
};

fn main() -> Result<()> {
    // Load the fee payer and API key from .env, then connect to devnet.
    let (payer, api_key) = env_config()?;
    let sender_seed = *payer.secret_bytes();
    let client = ZolanaClient::devnet(payer, &api_key);

    // Test setup: a test asset, the sender's funded private wallet, and the
    // recipient's private wallet.
    let (asset, registry, sender_keypair, sender_wallet) =
        setup_funded_wallet(&client, &sender_seed, 10_000)?;
    let (recipient, _recipient_keypair, mut recipient_wallet) =
        create_test_recipient(&client, registry)?;

    // Build and sign the private transfer. If the recipient does not have a
    // private wallet, the SDK resolves to a private-to-public withdrawal.
    let sender_address = Address::new_from_array(client.payer().pubkey().to_bytes());
    let asset_address = Address::new_from_array(asset.mint.to_bytes());
    let transfer = create_transfer_sync(CreateTransfer {
        rpc: client.rpc(),
        wallet: &sender_wallet,
        authority: &sender_keypair,
        owner_pubkey: Pubkey::default(),
        payer: sender_address,
        recipient: recipient.pubkey(),
        asset: asset_address, // for SOL: SOL_MINT
        amount: 4_000,
        memo: None, // encrypted note for the recipient
    })?;
    if transfer.recipient.is_public_withdrawal() {
        return Err(anyhow!(
            "expected a private transfer, got a public withdrawal"
        ));
    }

    // Prove and submit the private transfer. The proof shows the sender owns the
    // balance being spent and has not already spent it.
    let signature = Submit {
        indexer: client.indexer(),
        rpc: client.rpc(),
        prover: client.prover(),
        payer: client.payer(),
        tree: client.tree(),
        cu_limit: None,
    }
    .execute(
        transfer.signed,
        transfer.recipient.withdrawal().cloned(),
        transfer.wait_tag,
    )?;

    // Sync the recipient's private balance.
    sync_wallet(&mut recipient_wallet, client.indexer())?;
    let balance = get_private_token_balances(&recipient_wallet)?;

    println!("ok private transfer signature={signature} recipient_private_balance={balance:?}");
    Ok(())
}

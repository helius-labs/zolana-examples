use anyhow::{anyhow, Result};
use rust_client_example::{setup, SetupContext};
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{IndexerPollConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::{AssetDeposit, Deposit, DepositAsset};
use zolana_keypair::random_blinding;
use zolana_transaction::{Address, AssetRegistry, Wallet, SOL_MINT};
use zolana_user_registry_interface::{
    instruction::{register, set_merging_enabled, RegisterData},
    user_record_pda,
};
use zolana_wallet::{
    actions::{submit::MergeMaterial, transaction::is_plain_utxo},
    create_merge, submit_merge_transaction, sync_wallet_with_config, MergeParams,
    SubmitMergeTransaction, SyncWalletConfig,
};

const NOTE_AMOUNT: u64 = 100_000_000;
const NOTE_COUNT: usize = 3;
const TOTAL_AMOUNT: u64 = NOTE_AMOUNT * NOTE_COUNT as u64;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
        sender_solana,
        sender,
        ..
    } = setup()?;
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        &prover_url,
        tree,
    );
    let owner = sender_solana.pubkey();
    let sender_address = sender.shielded_address()?;

    let user_record = user_record_pda(&owner).0;
    let register_ix = register(
        user_record,
        owner,
        RegisterData {
            owner_p256: None, // uses Solana Ed25519
            nullifier_pubkey: sender_address.nullifier_pubkey,
            viewing_pubkey: *sender_address.viewing_pubkey.as_bytes(),
        },
    );
    let merge_enabled_ix = set_merging_enabled(user_record, owner, true);
    let setup_signature = client.create_and_send_transaction(
        &[register_ix, merge_enabled_ix],
        Address::new_from_array(owner.to_bytes()),
        &[&sender_solana],
    )?;
    println!("register wallet and enable merge tx={setup_signature}");

    let deposits = (0..NOTE_COUNT)
        .map(|_| {
            Ok(AssetDeposit {
                asset: DepositAsset::Sol,
                view_tag: sender_address.confidential_view_tag()?,
                owner: sender_address.owner_hash()?,
                blinding: random_blinding(),
                amount: NOTE_AMOUNT,
                utxo_data: None,
                memo: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let deposit_ix = Deposit {
        tree,
        depositor: owner,
        deposits,
    }
    .instruction()?;
    let deposit_signature = client.create_and_send_transaction(
        &[deposit_ix],
        Address::new_from_array(owner.to_bytes()),
        &[&sender_solana],
    )?;
    let deposit_slot = landed_slot(&client, deposit_signature)?;
    println!("deposit tx={deposit_signature}");

    let assets = AssetRegistry::default();
    let mut device_a = Wallet::new(sender_address, assets.clone())?;
    let mut device_b = Wallet::new(sender_address, assets)?;
    sync_wallet_with_config(
        &mut device_a,
        &sender,
        &client,
        SyncWalletConfig::at_slot(deposit_slot),
    )?;
    sync_wallet_with_config(
        &mut device_b,
        &sender,
        &client,
        SyncWalletConfig::at_slot(deposit_slot),
    )?;

    let device_a_balance = device_a.balance(SOL_MINT, None)?;
    let device_b_balance = device_b.balance(SOL_MINT, None)?;
    assert_eq!(device_a_balance.amount, TOTAL_AMOUNT);
    assert_eq!(device_a_balance.utxos.len(), NOTE_COUNT);
    assert_eq!(device_b_balance, device_a_balance);

    let mut shared_inputs = device_a
        .utxos
        .iter()
        .filter(|entry| !entry.spent && entry.utxo.asset == SOL_MINT && is_plain_utxo(entry))
        .map(|entry| entry.output_context.hash)
        .collect::<Vec<_>>();
    shared_inputs.sort();
    shared_inputs.truncate(2);
    assert_eq!(shared_inputs.len(), 2);
    assert!(shared_inputs.iter().all(|hash| {
        device_b
            .utxos
            .iter()
            .any(|entry| entry.output_context.hash == *hash && !entry.spent)
    }));

    let device_a_merge = create_merge(MergeParams {
        wallet: &device_a,
        keypair: &sender,
        asset: SOL_MINT,
        inputs: Some(shared_inputs.clone()),
    })?;
    let device_b_stale_merge = create_merge(MergeParams {
        wallet: &device_b,
        keypair: &sender,
        asset: SOL_MINT,
        inputs: Some(shared_inputs),
    })?;
    let material = MergeMaterial::from_keypair(&sender);

    let device_a_tree = device_a_merge.tree;
    let device_a_submitted = submit_merge_transaction(SubmitMergeTransaction {
        rpc: &client,
        indexer: &client,
        owner,
        payer: &sender_solana,
        material: &material,
        input_tree: device_a_tree,
        output_tree: tree,
        prover_url: &prover_url,
        prepared: device_a_merge.prepared,
    })?;
    IndexerPollConfig::default().poll_until(
        || {
            client.get_merkle_proofs(
                Address::new_from_array(tree.to_bytes()),
                vec![device_a_submitted.output_hash],
                None,
            )
        },
        |response| {
            response
                .proofs
                .iter()
                .any(|proof| proof.leaf == device_a_submitted.output_hash)
        },
    )?;
    let device_a_slot = landed_slot(&client, device_a_submitted.signature)?;
    println!("device A merge tx={}", device_a_submitted.signature);

    let device_b_tree = device_b_stale_merge.tree;
    let stale_error = match submit_merge_transaction(SubmitMergeTransaction {
        rpc: &client,
        indexer: &client,
        owner,
        payer: &sender_solana,
        material: &material,
        input_tree: device_b_tree,
        output_tree: tree,
        prover_url: &prover_url,
        prepared: device_b_stale_merge.prepared,
    }) {
        Ok(submitted) => {
            return Err(anyhow!(
                "stale device B merge unexpectedly succeeded: {}",
                submitted.signature
            ));
        }
        Err(error) => error,
    };
    println!("device B stale merge rejected: {stale_error}");

    sync_wallet_with_config(
        &mut device_b,
        &sender,
        &client,
        SyncWalletConfig::at_slot(device_a_slot),
    )?;
    let refreshed_balance = device_b.balance(SOL_MINT, None)?;
    assert_eq!(refreshed_balance.amount, TOTAL_AMOUNT);
    assert_eq!(refreshed_balance.utxos.len(), 2);

    let mut refreshed_inputs = device_b
        .utxos
        .iter()
        .filter(|entry| !entry.spent && entry.utxo.asset == SOL_MINT && is_plain_utxo(entry))
        .map(|entry| entry.output_context.hash)
        .collect::<Vec<_>>();
    refreshed_inputs.sort();

    // Resubmitting fetches fresh proofs and their current root_index values.
    let retry_merge = create_merge(MergeParams {
        wallet: &device_b,
        keypair: &sender,
        asset: SOL_MINT,
        inputs: Some(refreshed_inputs),
    })?;
    let retry_tree = retry_merge.tree;
    let retry_submitted = submit_merge_transaction(SubmitMergeTransaction {
        rpc: &client,
        indexer: &client,
        owner,
        payer: &sender_solana,
        material: &material,
        input_tree: retry_tree,
        output_tree: tree,
        prover_url: &prover_url,
        prepared: retry_merge.prepared,
    })?;
    IndexerPollConfig::default().poll_until(
        || {
            client.get_merkle_proofs(
                Address::new_from_array(tree.to_bytes()),
                vec![retry_submitted.output_hash],
                None,
            )
        },
        |response| {
            response
                .proofs
                .iter()
                .any(|proof| proof.leaf == retry_submitted.output_hash)
        },
    )?;
    let retry_slot = landed_slot(&client, retry_submitted.signature)?;
    sync_wallet_with_config(
        &mut device_b,
        &sender,
        &client,
        SyncWalletConfig::at_slot(retry_slot),
    )?;
    let final_balance = device_b.balance(SOL_MINT, None)?;
    assert_eq!(final_balance.amount, TOTAL_AMOUNT);
    assert_eq!(final_balance.utxos.len(), 1);
    println!("device B retry merge tx={}", retry_submitted.signature);

    Ok(())
}

fn landed_slot(client: &ZolanaClient<SolanaRpc>, signature: Signature) -> Result<u64> {
    client
        .get_signature_statuses(vec![signature])?
        .first()
        .and_then(|status| status.as_ref())
        .map(|status| status.slot)
        .ok_or_else(|| anyhow!("transaction status missing after confirmation"))
}

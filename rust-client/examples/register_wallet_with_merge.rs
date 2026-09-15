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

// Three deposits of 0.1 SOL each.
const NOTE_AMOUNT: u64 = 100_000_000;
const NOTE_COUNT: usize = 3;
const TOTAL_AMOUNT: u64 = NOTE_AMOUNT * 3;

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

    // Connect to Helius devnet RPC plus the Photon indexer and prover.
    let client = ZolanaClient::from_urls_allowing_insecure_http(
        SolanaRpc::new(rpc_url),
        &indexer_url,
        &prover_url,
        tree,
    );
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let owner = sender_solana.pubkey();
    let payer_address = Address::new_from_array(owner.to_bytes());
    let sender_address = sender.shielded_address()?;

    // Register the wallet and enable UTXO merge in one transaction.

    // 1. Build the registration instruction with the wallet's public keys.
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

    // 2. Enable merging for the same wallet.
    let merge_enabled_ix = set_merging_enabled(user_record, owner, true);

    // 3. Send and confirm like any Solana transaction.
    let setup_signature = client.create_and_send_transaction(
        &[register_ix, merge_enabled_ix],
        payer_address,
        &[&sender_solana],
    )?;
    println!("register wallet and enable merge tx={setup_signature}");

    // Deposit SOL into the sender's private balance.
    // A deposit from a public balance reveals sender, recipient, asset and amount.

    // 1. Move public SOL into three private token accounts (UTXOs).
    // The view tag is the sender's Solana public key in confidential rings.
    // Used by the indexer to fetch the sender's outputs.
    let deposit = AssetDeposit {
        asset: DepositAsset::Sol,
        view_tag: sender_address.confidential_view_tag()?,
        owner: sender_address.owner_hash()?,
        blinding: random_blinding(),
        amount: NOTE_AMOUNT,
        utxo_data: None,
        memo: None,
    };

    // Each deposit uses fresh blinding to create a distinct note commitment.
    let deposits = vec![
        deposit.clone(),
        AssetDeposit {
            blinding: random_blinding(),
            ..deposit.clone()
        },
        AssetDeposit {
            blinding: random_blinding(),
            ..deposit
        },
    ];
    let deposit_ix = Deposit {
        tree,
        depositor: owner,
        deposits,
    }
    .instruction()?;

    // 2. Send and confirm like any Solana transaction.
    let deposit_signature =
        client.create_and_send_transaction(&[deposit_ix], payer_address, &[&sender_solana])?;
    let deposit_slot = landed_slot(&client, deposit_signature)?;
    println!("deposit tx={deposit_signature}");

    // 3. Sync two devices to the same private wallet, gated on the deposit's slot.
    // Each device decrypts the transaction outputs locally to read the private balance.
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

    // Merge two private token accounts into one without changing the private balance.
    // Merge requires the nullifier key and decrypted UTXOs.
    // Merge cannot change the owner or spend the balance.

    // 1. Select private token accounts (UTXOs) that make up the private balance for the merge.
    let mut shared_inputs = Vec::new();
    for entry in &device_a.utxos {
        if !entry.spent && entry.utxo.asset == SOL_MINT && is_plain_utxo(entry) {
            shared_inputs.push(entry.output_context.hash);
        }
    }
    shared_inputs.sort();
    shared_inputs.truncate(2);
    assert_eq!(shared_inputs.len(), 2);

    let mut device_b_inputs = Vec::new();
    for entry in &device_b.utxos {
        if !entry.spent {
            device_b_inputs.push(entry.output_context.hash);
        }
    }

    for hash in &shared_inputs {
        assert!(
            device_b_inputs.contains(hash),
            "shared input missing from device B"
        );
    }

    // 2. Build both devices' merges from the same inputs before either is submitted.
    // This creates the two-device conflict demonstrated below.
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

    // 3. Send and confirm like any Solana transaction.
    let request = SubmitMergeTransaction {
        rpc: &client,
        indexer: &client,
        owner,
        payer: &sender_solana,
        material: &material,
        input_tree: device_a_merge.tree,
        output_tree: tree,
        prover_url: &prover_url,
        prepared: device_a_merge.prepared,
    };
    let device_a_submitted = submit_merge_transaction(request)?;

    // Wait until the indexer can return a Merkle proof for the merged output.
    wait_for_indexed_output(&client, tree, device_a_submitted.output_hash)?;

    let device_a_slot = landed_slot(&client, device_a_submitted.signature)?;
    println!("device A merge tx={}", device_a_submitted.signature);

    // 4. Submit Device B's stale merge and expect rejection.
    // Device A has already spent the shared input nullifiers.
    let device_b_tree = device_b_stale_merge.tree;
    let request = SubmitMergeTransaction {
        rpc: &client,
        indexer: &client,
        owner,
        payer: &sender_solana,
        material: &material,
        input_tree: device_b_tree,
        output_tree: tree,
        prover_url: &prover_url,
        prepared: device_b_stale_merge.prepared,
    };
    let result = submit_merge_transaction(request);
    let stale_error = match result {
        Ok(submitted) => {
            return Err(anyhow!(
                "stale device B merge unexpectedly succeeded: {}",
                submitted.signature
            ));
        }
        Err(error) => error,
    };
    println!("device B stale merge rejected: {stale_error}");

    // Recover Device B's stale wallet state and retry the merge.

    // 1. Fetch the sender's outputs again, gated on Device A's merge slot,
    // and read the remaining private balance.
    sync_wallet_with_config(
        &mut device_b,
        &sender,
        &client,
        SyncWalletConfig::at_slot(device_a_slot),
    )?;
    let refreshed_balance = device_b.balance(SOL_MINT, None)?;
    assert_eq!(refreshed_balance.amount, TOTAL_AMOUNT);
    assert_eq!(refreshed_balance.utxos.len(), 2);

    // 2. Select the remaining unspent UTXOs from the refreshed wallet.
    let mut refreshed_inputs = Vec::new();
    for entry in &device_b.utxos {
        if !entry.spent && entry.utxo.asset == SOL_MINT && is_plain_utxo(entry) {
            refreshed_inputs.push(entry.output_context.hash);
        }
    }
    refreshed_inputs.sort();

    // 3. Rebuild the merge from the refreshed wallet.
    // Do not reuse the merge prepared from stale wallet state.
    let retry_merge = create_merge(MergeParams {
        wallet: &device_b,
        keypair: &sender,
        asset: SOL_MINT,
        inputs: Some(refreshed_inputs),
    })?;
    // 4. Send and confirm like any Solana transaction.
    // Submission fetches fresh Merkle proofs and current root_index values.
    let request = SubmitMergeTransaction {
        rpc: &client,
        indexer: &client,
        owner,
        payer: &sender_solana,
        material: &material,
        input_tree: retry_merge.tree,
        output_tree: tree,
        prover_url: &prover_url,
        prepared: retry_merge.prepared,
    };
    let retry_submitted = submit_merge_transaction(request)?;
    wait_for_indexed_output(&client, tree, retry_submitted.output_hash)?;

    // 5. Fetch the sender's outputs again, gated on the retry's slot,
    // and check that one UTXO holds the original 0.3 SOL balance.
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

fn wait_for_indexed_output(
    client: &ZolanaClient<SolanaRpc>,
    tree: Address,
    output_hash: [u8; 32],
) -> Result<()> {
    let tree = Address::new_from_array(tree.to_bytes());
    IndexerPollConfig::default().poll_until(
        || client.get_merkle_proofs(tree, vec![output_hash], None),
        |response| {
            for proof in &response.proofs {
                if proof.leaf == output_hash {
                    return true;
                }
            }
            false
        },
    )?;
    Ok(())
}

fn landed_slot(client: &ZolanaClient<SolanaRpc>, signature: Signature) -> Result<u64> {
    let statuses = client.get_signature_statuses(vec![signature])?;
    match statuses.first() {
        Some(Some(status)) => Ok(status.slot),
        _ => Err(anyhow!("transaction status missing after confirmation")),
    }
}

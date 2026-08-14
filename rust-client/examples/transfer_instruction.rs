use anyhow::{anyhow, Result};
use rust_client_example::env_config;
use solana_keypair::Keypair;
use solana_signature::Signature;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::{AssetDeposit, Deposit, DepositAsset, Transact};
use zolana_keypair::{random_blinding, ShieldedKeypair};
use zolana_transaction::{
    decrypt_transactions,
    instructions::{transact::ConfidentialTransfer, types::SppProofInputUtxo},
    AssetRegistry, SOL_MINT,
};

const DEPOSIT_AMOUNT: u64 = 1_000_000_000;
const TRANSFER_AMOUNT: u64 = 300_000_000;

fn main() -> Result<()> {
    let cfg = env_config()?;
    let rpc_url = cfg.rpc_url;
    let indexer_url = cfg.indexer_url;
    let prover_url = cfg.prover_url;
    let tree = cfg.tree;
    let sender = ShieldedKeypair::from_solana_keypair(&cfg.payer)?;
    let recipient_address =
        ShieldedKeypair::from_solana_keypair(&Keypair::new())?.shielded_address()?;

    // Load the funded fee payer and localnet settings, then connect.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url, tree)?;

    // Mints that are registered with Solana Rings for privacy.
    let assets = AssetRegistry::default();
    // SPL: assets.insert(spl.asset_id, spl.mint)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender_solana_keypair = sender.to_solana_keypair()?;
    let sender_shielded_address = sender.shielded_address()?;

    // Deposit SOL into the sender's private balance.
    // A deposit from a public balance reveals
    // sender, recipient, asset and amount.
    // Alternatively, you can onramp fiat directly to a private balance.

    // 1. Move public SOL into the sender's private balance.
    let sender_balances_after_deposit = {
        let deposit_ix = Deposit {
            tree,
            depositor: sender_solana_keypair.pubkey(),
            deposits: vec![AssetDeposit {
                asset: DepositAsset::Sol,
                // SPL: asset: DepositAsset::Spl(zolana_interface::instruction::DepositSplAccounts {
                // SPL:     mint: spl.mint,
                // SPL:     user_token: spl.user_token_account,
                // SPL:     token_program: spl.token_program,
                // SPL: }),
                view_tag: sender_shielded_address.confidential_view_tag()?,
                owner: sender_shielded_address.owner_hash()?,
                blinding: random_blinding(),
                amount: DEPOSIT_AMOUNT,
                utxo_data: None,
                memo: None,
            }],
        }
        .instruction()?;

        // 2. Send and confirm like any Solana transaction; the landed slot gates
        // the indexer fetch below.
        let signature = client.create_and_send_transaction(
            &[deposit_ix],
            sender_solana_keypair.pubkey(),
            &[&sender_solana_keypair],
        )?;
        let slot = landed_slot(&client, signature)?;

        // 3. Fetch transaction outputs from the indexer, gated on the deposit's slot.
        // The indexer returns encrypted outputs by view tag, the sender's public key in Confidential Rings.
        let sender_tag = sender_shielded_address.confidential_view_tag()?;
        let response = client.get_shielded_transactions_by_tags(
            vec![sender_tag],
            None,
            Some(50),
            Some(IndexerRpcConfig::at_slot(slot)),
        )?;

        // 4. The sender decrypts the transaction outputs locally to update the private balance.
        let balances = decrypt_transactions(&sender, &response.transactions, &assets)
            .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;

        let sender_balance = balances
            .get_balance(SOL_MINT)
            // SPL: .get_balance(spl.mint)
            .expect("failed to fetch sender's utxo");
        assert_eq!(sender_balance.amount, DEPOSIT_AMOUNT);
        assert_eq!(sender_balance.utxos.len(), 1);

        balances
    };

    // Confidential SOL transfer to the recipient's private balance.
    // A confidential transfer reveals only sender and recipient,
    // not the asset or amount.
    let sender_balances_after_transfer = {
        // 1. Select UTXOs that make up the private balance for the transfer.
        let transfer_utxo = sender_balances_after_deposit
            .get_balance(SOL_MINT)
            // SPL: .get_balance(spl.mint)
            .and_then(|balance| balance.utxos.first())
            .expect("failed to fetch deposited utxo")
            .clone();

        // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
        let transfer_input_utxo = SppProofInputUtxo::new(transfer_utxo, &sender);

        // 3. Build and sign the confidential transfer.
        // Signing encrypts the asset and amount and produces the proof inputs for the ZK prover.
        let mut transfer = ConfidentialTransfer::new(
            sender_shielded_address,
            vec![transfer_input_utxo],
            sender_solana_keypair.pubkey(),
        );
        transfer.send(&recipient_address, SOL_MINT, TRANSFER_AMOUNT)?;
        // SPL: transfer.send(&recipient_address, spl.mint, TRANSFER_AMOUNT)?;
        let proof_inputs = transfer.sign(&sender, &assets)?;

        // 4. Fetch the zk proof to prove the sender can spend the balance without revealing asset and amount.
        let transfer_data = client.prove_transact(tree, proof_inputs, None)?;

        // 5. Construct the instruction.
        let transfer_ix = Transact {
            payer: sender_solana_keypair.pubkey(),
            input_tree: tree,
            output_tree: tree,
            owner_signers: Vec::new(),
            interface_transfer_accounts: Vec::new(),
            data: transfer_data,
        }
        .instruction();

        // 6. Send and confirm like any Solana transaction; confirmation yields the landed slot.
        let signature = client.create_and_send_transaction(
            &[transfer_ix],
            sender_solana_keypair.pubkey(),
            &[&sender_solana_keypair],
        )?;
        let slot = landed_slot(&client, signature)?;

        // 7. Sync the sender's wallet, gated on the transfer's slot, and read
        // the remaining private balance.
        let sender_tag = sender_shielded_address.confidential_view_tag()?;
        let response = client.get_shielded_transactions_by_tags(
            vec![sender_tag],
            None,
            Some(50),
            Some(IndexerRpcConfig::at_slot(slot)),
        )?;
        let sender_balances = decrypt_transactions(&sender, &response.transactions, &assets)
            .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;
        let sender_balance = sender_balances
            .get_balance(SOL_MINT)
            // SPL: .get_balance(spl.mint)
            .expect("failed to fetch sender's utxo");
        assert_eq!(sender_balance.amount, DEPOSIT_AMOUNT - TRANSFER_AMOUNT);
        assert_eq!(sender_balance.utxos.len(), 1);

        sender_balances
    };

    println!(
        "ok private transfer remaining_private_sol={}",
        sender_balances_after_transfer
            .get_balance(SOL_MINT)
            .expect("failed to fetch sender's utxo")
            .amount
    );
    Ok(())
}

/// Slot the confirmed transaction landed in, which drives the indexer
/// freshness gate on the fetches that read the transaction back.
fn landed_slot(client: &ZolanaClient<SolanaRpc>, signature: Signature) -> Result<u64> {
    client
        .get_signature_statuses(vec![signature])?
        .first()
        .and_then(|status| status.as_ref())
        .map(|status| status.slot)
        .ok_or_else(|| anyhow!("transaction status missing after confirmation"))
}

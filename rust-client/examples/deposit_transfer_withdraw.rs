use anyhow::{anyhow, Result};
use rust_client_example::{cli_keypair, landed_slot, setup, SetupContext};
use solana_keypair::Keypair;
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::instruction::{
    AssetDeposit, Deposit, DepositAsset, Transact, TransactInterfaceTransferAccounts,
    TransactSolTransferAccounts,
};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{
    decrypt_spendable, instructions::transact::ConfidentialTransaction, AssetRegistry, SOL_MINT,
};

const DEPOSIT_AMOUNT: u64 = 10_000_000;
const TRANSFER_AMOUNT: u64 = 3_000_000;
const WITHDRAW_AMOUNT: u64 = 3_000_000;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;

    // Connect to the RPC, indexer, and prover.
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;

    // Mints that are registered with Solana Rings for privacy.
    let assets = AssetRegistry::default();
    // SPL: assets.insert(spl.asset_id, spl.mint)?;

    // Initialize the sender's private wallet and local authority
    // to decrypt transactions and sync balances.
    // The Solana signer and private wallet are derived from the same Ed25519 seed.
    let sender = ShieldedKeypair::from_keypair(&cli_keypair()?)?;
    let recipient = ShieldedKeypair::from_keypair(&Keypair::new())?;
    let sender_shielded_address = sender.shielded_address()?;

    // Deposit SOL into the sender's private balance.
    // A deposit from a public balance reveals
    // sender, recipient, asset and amount.
    // Alternatively, you can onramp fiat directly to a private balance.

    // 1. Move public SOL into the sender's private balance.
    let sender_balances_after_deposit = {
        let deposit_ix = Deposit {
            tree,
            depositor: sender.pubkey(),
            deposits: vec![AssetDeposit {
                asset: DepositAsset::Sol,
                // SPL: asset: DepositAsset::Spl(zolana_interface::instruction::DepositSplAccounts {
                // SPL:     mint: spl.mint,
                // SPL:     user_token: spl.user_token_account,
                // SPL:     token_program: spl.token_program,
                // SPL: }),
                view_tag: sender_shielded_address.confidential_view_tag()?,
                owner: sender_shielded_address.owner_hash()?,
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
            sender.pubkey(),
            &[&sender],
            client.compute_budget(),
        )?;
        let slot = landed_slot(&client, signature)?;

        // 3. Fetch this transaction's outputs, gated on its confirmed slot.
        let response = client.get_shielded_transactions_by_signature(
            signature,
            Some(IndexerRpcConfig::at_slot(slot)),
        )?;
        let transactions = response
            .transactions
            .into_iter()
            .map(|indexed| indexed.transaction)
            .collect::<Vec<_>>();

        // 4. The sender decrypts the transaction outputs locally to read the funds deposited in this run.
        let balances = decrypt_spendable(&sender, &transactions, &assets)
            .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?
            .balances;

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
        let mut transfer = ConfidentialTransaction::new(vec![transfer_utxo], sender.pubkey())?;

        // 3. Build and encrypt the confidential transfer.
        // Encryption hides the asset and amount and produces the proof inputs for the ZK prover.
        transfer.transfer_sol(&recipient.shielded_address()?, TRANSFER_AMOUNT)?;
        // SPL: transfer.transfer(&recipient.shielded_address()?, spl.mint, TRANSFER_AMOUNT)?;
        let proof_inputs = transfer.encrypt(&sender)?;

        // 4. Fetch the zk proof to prove the sender can spend the balance without revealing asset and amount.
        let transfer_data = client.prove_transact(proof_inputs, None, &sender)?;

        // 5. Construct the instruction.
        let transfer_ix = Transact {
            payer: sender.pubkey(),
            input_trees: vec![tree],
            output_tree: tree,
            owner_signers: Vec::new(),
            interface_transfer_accounts: Vec::new(),
            data: transfer_data,
        }
        .instruction();

        // 6. Send and confirm like any Solana transaction; confirmation yields the landed slot.
        let signature = client.create_and_send_transaction(
            &[transfer_ix],
            sender.pubkey(),
            &[&sender],
            client.compute_budget(),
        )?;
        let slot = landed_slot(&client, signature)?;

        // 7. Fetch this transaction's outputs, gated on its confirmed slot.
        let response = client.get_shielded_transactions_by_signature(
            signature,
            Some(IndexerRpcConfig::at_slot(slot)),
        )?;
        let transactions = response
            .transactions
            .into_iter()
            .map(|indexed| indexed.transaction)
            .collect::<Vec<_>>();
        let sender_balances = decrypt_spendable(&sender, &transactions, &assets)
            .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?
            .balances;
        let sender_balance = sender_balances
            .get_balance(SOL_MINT)
            // SPL: .get_balance(spl.mint)
            .expect("failed to fetch sender's utxo");
        assert_eq!(sender_balance.amount, DEPOSIT_AMOUNT - TRANSFER_AMOUNT);
        assert_eq!(sender_balance.utxos.len(), 1);

        sender_balances
    };

    // Withdraw SOL back to the sender's public balance.
    // A withdrawal from a confidential balance reveals
    // sender, recipient, asset and amount.
    {
        // 1. Select UTXOs that make up the private balance for the withdrawal.
        let withdrawal_utxo = sender_balances_after_transfer
            .get_balance(SOL_MINT)
            // SPL: .get_balance(spl.mint)
            .and_then(|balance| balance.utxos.first())
            .expect("failed to fetch sender's utxo")
            .clone();

        // 2. Prepare the selected UTXOs as inputs for the zero-knowledge proof.
        let mut withdrawal = ConfidentialTransaction::new(vec![withdrawal_utxo], sender.pubkey())?;

        // 3. Build and encrypt the confidential withdrawal.
        // Encryption hides the private change and produces the ZK prover inputs.
        withdrawal.withdraw_sol(WITHDRAW_AMOUNT, sender.pubkey())?;
        // SPL: withdrawal.withdraw(spl.mint, WITHDRAW_AMOUNT, spl.user_token_account)?;
        let proof_inputs = withdrawal.encrypt(&sender)?;

        // 4. Fetch the ZK proof to prove the sender can spend the balance.
        let withdrawal_data = client.prove_transact(proof_inputs, None, &sender)?;

        // 5. Combine the proof and withdrawal accounts in a single instruction.
        let withdraw_ix = Transact {
            payer: sender.pubkey(),
            input_trees: vec![tree],
            output_tree: tree,
            owner_signers: Vec::new(),
            interface_transfer_accounts: vec![TransactInterfaceTransferAccounts::Sol(
                TransactSolTransferAccounts {
                    recipient: sender.pubkey(),
                },
            )],
            // SPL: interface_transfer_accounts: vec![
            // SPL:     TransactInterfaceTransferAccounts::SplWithdrawal(
            // SPL:         zolana_interface::instruction::TransactSplWithdrawalAccounts {
            // SPL:             mint: spl.mint,
            // SPL:             vault: spl.vault,
            // SPL:             user_token_account: spl.user_token_account,
            // SPL:             token_program: spl.token_program,
            // SPL:         },
            // SPL:     ),
            // SPL: ],
            data: withdrawal_data,
        }
        .instruction();

        // 6. Send and confirm like any Solana transaction.
        let signature = client.create_and_send_transaction(
            &[withdraw_ix],
            sender.pubkey(),
            &[&sender],
            client.compute_budget(),
        )?;
        let slot = landed_slot(&client, signature)?;

        // 7. Fetch this transaction's outputs, gated on its confirmed slot.
        let response = client.get_shielded_transactions_by_signature(
            signature,
            Some(IndexerRpcConfig::at_slot(slot)),
        )?;
        let transactions = response
            .transactions
            .into_iter()
            .map(|indexed| indexed.transaction)
            .collect::<Vec<_>>();
        let sender_balances = decrypt_spendable(&sender, &transactions, &assets)
            .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?
            .balances;
        let sender_balance = sender_balances
            .get_balance(SOL_MINT)
            // SPL: .get_balance(spl.mint)
            .expect("failed to fetch sender's utxo");
        assert_eq!(
            sender_balance.amount,
            DEPOSIT_AMOUNT - TRANSFER_AMOUNT - WITHDRAW_AMOUNT
        );
        assert_eq!(sender_balance.utxos.len(), 1);

        // 8. Read the funds remaining from this run and the public SOL balance.
        let solana_balance = client.get_balance(sender.pubkey())?;
        println!("withdraw solana_balance={solana_balance} tx={signature}");
        // SPL: println!(
        // SPL:     "withdraw user_token={} tx={signature}",
        // SPL:     spl.user_token_account,
        // SPL: );
    }
    Ok(())
}

use anyhow::{anyhow, ensure, Result};
use rust_client_example::{cli_keypair, setup, SetupContext};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_system_interface::instruction::create_account;
use spl_token_interface::instruction::{initialize_account3, initialize_mint2, mint_to};
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::{
    instruction::{AssetDeposit, CreateSplInterface, Deposit, DepositAsset, DepositSplAccounts},
    pda,
    state::SplAssetRegistry,
    SPL_TOKEN_ACCOUNT_LEN, SPL_TOKEN_MINT_ACCOUNT_LEN, SPL_TOKEN_PROGRAM_ID,
};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{decrypt_transactions, AssetRegistry};

const DEPOSIT_AMOUNT: u64 = 1_000_000_000;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url, tree)?;
    let sender_solana_keypair = cli_keypair()?;
    let sender = ShieldedKeypair::from_keypair(&sender_solana_keypair)?;
    let sender_pubkey = sender_solana_keypair.pubkey();
    let sender_shielded_address = sender.shielded_address()?;

    // Prepare a test mint and public tokens before the interface setup and deposit.
    let mint = Keypair::new();
    let source_token = Keypair::new();
    let token_program = Address::new_from_array(SPL_TOKEN_PROGRAM_ID);
    let mint_rent = client.get_minimum_balance_for_rent_exemption(SPL_TOKEN_MINT_ACCOUNT_LEN)?;
    let token_rent = client.get_minimum_balance_for_rent_exemption(SPL_TOKEN_ACCOUNT_LEN)?;
    client.create_and_send_transaction(
        &[
            create_account(
                &sender_pubkey,
                &mint.pubkey(),
                mint_rent,
                SPL_TOKEN_MINT_ACCOUNT_LEN as u64,
                &token_program,
            ),
            initialize_mint2(&token_program, &mint.pubkey(), &sender_pubkey, None, 9)?,
            create_account(
                &sender_pubkey,
                &source_token.pubkey(),
                token_rent,
                SPL_TOKEN_ACCOUNT_LEN as u64,
                &token_program,
            ),
            initialize_account3(
                &token_program,
                &source_token.pubkey(),
                &mint.pubkey(),
                &sender_pubkey,
            )?,
            mint_to(
                &token_program,
                &mint.pubkey(),
                &source_token.pubkey(),
                &sender_pubkey,
                &[],
                DEPOSIT_AMOUNT,
            )?,
        ],
        sender_pubkey,
        &[&sender_solana_keypair, &mint, &source_token],
        client.compute_budget(),
    )?;

    // 1. Fetch the interface PDA. A fresh mint has no interface yet.
    let vault = pda::spl_interface(&mint.pubkey());
    ensure!(
        client.get_account(vault)?.is_none(),
        "expected the test mint's interface PDA to be absent"
    );

    // 2. Create the mint registry PDA and token vault. The sender pays their rent.
    let create_interface_ix = CreateSplInterface {
        authority: sender_pubkey,
        mint: mint.pubkey(),
        token_program,
    }
    .instruction();

    // 3. Move public tokens into the sender's private balance.
    // A deposit from a public balance reveals sender, recipient, asset and amount.
    let sender_view_tag = sender_shielded_address.confidential_view_tag()?;
    let deposit_ix = Deposit {
        tree,
        depositor: sender_pubkey,
        deposits: vec![AssetDeposit {
            asset: DepositAsset::Spl(DepositSplAccounts {
                mint: mint.pubkey(),
                user_token: source_token.pubkey(),
                token_program,
            }),
            view_tag: sender_view_tag,
            owner: sender_shielded_address.owner_hash()?,
            amount: DEPOSIT_AMOUNT,
            utxo_data: None,
            memo: None,
        }],
    }
    .instruction()?;

    // 4. Send both instructions in one transaction; confirmation yields the landed slot.
    let signature = client.create_and_send_transaction(
        &[create_interface_ix, deposit_ix],
        sender_pubkey,
        &[&sender_solana_keypair],
        client.compute_budget(),
    )?;
    let slot = client
        .get_signature_statuses(vec![signature])?
        .first()
        .and_then(|status| status.as_ref())
        .map(|status| status.slot)
        .ok_or_else(|| anyhow!("transaction status missing after confirmation"))?;

    // 5. Register the assigned asset ID for the SDK's balance lookup.
    let registry_account = client
        .get_account(pda::spl_asset_registry(&mint.pubkey()))?
        .ok_or_else(|| anyhow!("mint registry missing after interface setup"))?;
    let registry = SplAssetRegistry::from_account_bytes(&registry_account.data)
        .map_err(|e| anyhow!("decode mint registry: {e:?}"))?;
    let mut assets = AssetRegistry::default();
    assets.insert(registry.asset_id, registry.mint)?;

    // 6. Fetch this transaction's outputs, gated on its confirmed slot.
    let response = client
        .get_shielded_transactions_by_signature(signature, Some(IndexerRpcConfig::at_slot(slot)))?;
    let transactions = response
        .transactions
        .into_iter()
        .map(|indexed| indexed.transaction)
        .collect::<Vec<_>>();

    // 7. The sender decrypts the transaction outputs locally to read the private balance.
    let balances_after_deposit = decrypt_transactions(&sender, &transactions, &assets)
        .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?;
    let deposit_balance = balances_after_deposit
        .get_balance(mint.pubkey())
        .ok_or_else(|| anyhow!("failed to fetch sender's utxo"))?;
    assert_eq!(deposit_balance.amount, DEPOSIT_AMOUNT);
    assert_eq!(deposit_balance.utxos.len(), 1);
    println!(
        "deposit mint={} private_balance={} tx={signature}",
        mint.pubkey(),
        deposit_balance.amount,
    );
    Ok(())
}

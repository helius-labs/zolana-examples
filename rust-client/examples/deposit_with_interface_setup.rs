use anyhow::{anyhow, Result};
use rust_client_example::{
    cli_keypair, landed_slot, setup, setup_test_token, SetupContext, TestToken,
};
use solana_signer::Signer;
use zolana_client::{IndexerRpcConfig, Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::{
    instruction::{AssetDeposit, CreateSplInterface, Deposit, DepositAsset, DepositSplAccounts},
    pda,
    state::SplAssetRegistry,
};
use zolana_keypair::ShieldedKeypair;
use zolana_transaction::{decrypt_spendable, AssetRegistry};

const DEPOSIT_AMOUNT: u64 = 1_000_000_000;

fn main() -> Result<()> {
    let SetupContext {
        rpc_url,
        indexer_url,
        prover_url,
        tree,
    } = setup()?;
    let client = ZolanaClient::from_urls(SolanaRpc::new(rpc_url), &indexer_url, prover_url)?;
    let sender_solana_keypair = cli_keypair()?;
    let sender = ShieldedKeypair::from_keypair(&sender_solana_keypair)?;
    let sender_pubkey = sender_solana_keypair.pubkey();
    let sender_shielded_address = sender.shielded_address()?;

    let TestToken { mint, source_token } =
        setup_test_token(&client, &sender_solana_keypair, DEPOSIT_AMOUNT)?;
    let token_program = pda::spl_token_program_id();

    // 1. Check whether the mint already has an interface PDA, the escrow that holds deposited tokens.
    // A second create fails the transaction, so skip step 2 when the account exists.
    let interface_address = pda::spl_interface(&mint);
    let interface_account = client.get_account(interface_address)?;
    let mut instructions = Vec::new();

    // 2. Create the mint registry PDA and interface token account.
    // The authority signer pays their rent.
    if interface_account.is_none() {
        let create_interface_ix = CreateSplInterface {
            authority: sender_pubkey,
            mint,
            token_program,
        }
        .instruction();
        instructions.push(create_interface_ix);
    }

    // 3. Move public tokens into the sender's private balance.
    // A deposit from a public balance reveals sender, recipient, asset and amount.
    let deposit_ix = Deposit {
        tree,
        depositor: sender_pubkey,
        deposits: vec![AssetDeposit {
            asset: DepositAsset::Spl(DepositSplAccounts {
                mint,
                user_token: source_token,
                token_program,
            }),
            view_tag: sender_shielded_address.confidential_view_tag()?,
            owner: sender_shielded_address.owner_hash()?,
            amount: DEPOSIT_AMOUNT,
            utxo_data: None,
            memo: None,
        }],
    }
    .instruction()?;

    // 4. Send the instructions in one transaction.
    instructions.push(deposit_ix);
    let signature = client.create_and_send_transaction(
        &instructions,
        sender_pubkey,
        &[&sender_solana_keypair],
        client.compute_budget(),
    )?;
    let slot = landed_slot(&client, signature)?;

    // 5. Register the assigned asset ID for the SDK's balance lookup.
    let registry_account = client
        .get_account(pda::spl_asset_registry(&mint))?
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
    let balances = decrypt_spendable(&sender, &transactions, &assets)
        .map_err(|e| anyhow!("decrypt sender transactions: {e:?}"))?
        .balances;
    let deposit_balance = balances
        .get_balance(mint)
        .ok_or_else(|| anyhow!("failed to fetch sender's utxo"))?;
    assert_eq!(deposit_balance.amount, DEPOSIT_AMOUNT);
    assert_eq!(deposit_balance.utxos.len(), 1);
    println!(
        "deposit mint={} private_balance={} tx={signature}",
        mint, deposit_balance.amount,
    );
    Ok(())
}

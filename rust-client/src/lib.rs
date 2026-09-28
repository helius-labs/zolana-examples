//! Shared helpers for the examples: the fee payer and funded test tokens.

use anyhow::{anyhow, Result};
use solana_address::Address;
use solana_keypair::{read_keypair_file, Keypair};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction::create_account;
use spl_token_interface::instruction::{initialize_account3, initialize_mint2, mint_to};
use zolana_client::{Rpc, SolanaRpc, ZolanaClient};
use zolana_interface::{pda, SPL_TOKEN_ACCOUNT_LEN, SPL_TOKEN_MINT_ACCOUNT_LEN};

/// The Solana CLI wallet (`ZOLANA_PAYER_KEYPAIR`, defaults to
/// `~/.config/solana/id.json`).
pub fn cli_keypair() -> Result<Keypair> {
    let path = std::env::var("ZOLANA_PAYER_KEYPAIR")
        .unwrap_or_else(|_| "~/.config/solana/id.json".to_string());
    let path = shellexpand::tilde(&path).into_owned();
    read_keypair_file(&path).map_err(|e| anyhow!("load keypair {path}: {e}"))
}

/// Test mint and the payer's funded token account.
pub struct TestToken {
    pub mint: Address,
    pub source_token: Address,
}

/// Create a test mint and fund the payer's token account.
pub fn setup_test_token(
    client: &ZolanaClient<SolanaRpc>,
    payer: &Keypair,
    amount: u64,
) -> Result<TestToken> {
    let payer_pubkey = payer.pubkey();
    let mint = Keypair::new();
    let source_token = Keypair::new();
    let token_program = pda::spl_token_program_id();
    let mint_rent = client.get_minimum_balance_for_rent_exemption(SPL_TOKEN_MINT_ACCOUNT_LEN)?;
    let token_rent = client.get_minimum_balance_for_rent_exemption(SPL_TOKEN_ACCOUNT_LEN)?;
    client.create_and_send_transaction(
        &[
            create_account(
                &payer_pubkey,
                &mint.pubkey(),
                mint_rent,
                SPL_TOKEN_MINT_ACCOUNT_LEN as u64,
                &token_program,
            ),
            initialize_mint2(&token_program, &mint.pubkey(), &payer_pubkey, None, 9)?,
            create_account(
                &payer_pubkey,
                &source_token.pubkey(),
                token_rent,
                SPL_TOKEN_ACCOUNT_LEN as u64,
                &token_program,
            ),
            initialize_account3(
                &token_program,
                &source_token.pubkey(),
                &mint.pubkey(),
                &payer_pubkey,
            )?,
            mint_to(
                &token_program,
                &mint.pubkey(),
                &source_token.pubkey(),
                &payer_pubkey,
                &[],
                amount,
            )?,
        ],
        payer_pubkey,
        &[payer, &mint, &source_token],
        client.compute_budget(),
    )?;

    Ok(TestToken {
        mint: mint.pubkey(),
        source_token: source_token.pubkey(),
    })
}

/// Slot the confirmed transaction landed in, which drives the indexer
/// freshness gate on the fetches that read the transaction back.
pub fn landed_slot(client: &ZolanaClient<SolanaRpc>, signature: Signature) -> Result<u64> {
    client
        .get_signature_statuses(vec![signature])?
        .first()
        .and_then(|status| status.as_ref())
        .map(|status| status.slot)
        .ok_or_else(|| anyhow!("transaction status missing after confirmation"))
}

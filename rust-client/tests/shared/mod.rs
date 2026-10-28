use anyhow::{ensure, Result};
use rust_client_example::SetupContext;
use solana_address::Address;
use solana_keypair::{write_keypair_file, Keypair};
use spr_client::{ProofDataSource, SolanaRpc, ZolanaClient};
use std::{io::Write, path::Path, process::Command};

const EXAMPLES: &[&str] = &[
    "register_private_wallet",
    "deposit_transfer_withdraw",
    "deposit_with_interface_setup",
    "sync_balance",
    "read_history",
];

// Reuse the example flows; their standalone mains are unused in this test binary.
#[allow(dead_code)]
#[path = "../../examples/register_private_wallet.rs"]
mod register_private_wallet;

#[allow(dead_code)]
#[path = "../../examples/deposit_transfer_withdraw.rs"]
mod deposit_transfer_withdraw;

#[allow(dead_code)]
#[path = "../../examples/deposit_with_interface_setup.rs"]
mod deposit_with_interface_setup;

#[allow(dead_code)]
#[path = "../../examples/sync_balance.rs"]
mod sync_balance;

#[allow(dead_code)]
#[path = "../../examples/read_history.rs"]
mod read_history;

pub fn run_rust_examples(
    client: &ZolanaClient<SolanaRpc>,
    tree: Address,
    payer: &Keypair,
) -> Result<()> {
    use spr_test_utils::wallet::{
        PrivateTransactionDirection as Direction, PrivateTransactionKind as Kind,
    };
    use spr_transaction::SOL_MINT;

    register_private_wallet::run(client, payer)?;
    deposit_transfer_withdraw::run(client, tree, payer)?;
    let mint = deposit_with_interface_setup::run(client, tree, payer)?;
    let balances = sync_balance::run(client, payer)?;
    assert_eq!(
        (
            balances.get_balance(SOL_MINT).map(|balance| balance.amount),
            balances.get_balance(mint).map(|balance| balance.amount)
        ),
        (Some(4_000_000), Some(1_000_000_000)),
        "balance sync must recover both the SOL change and the deposited SPL tokens",
    );
    let history = read_history::run(client, payer)?;
    assert_eq!(
        history
            .iter()
            .map(|tx| (tx.kind, tx.direction, tx.asset, tx.amount))
            .collect::<Vec<_>>(),
        vec![
            (Kind::Deposit, Direction::Inbound, SOL_MINT, 10_000_000),
            (
                Kind::PrivateTransfer,
                Direction::Outbound,
                SOL_MINT,
                3_000_000
            ),
            (
                Kind::PublicWithdrawal,
                Direction::Outbound,
                SOL_MINT,
                3_000_000
            ),
            (Kind::Deposit, Direction::Inbound, mint, 1_000_000_000),
        ],
        "history must recover every operation and both assets",
    );
    Ok(())
}

pub fn run_typescript_examples(
    context: &SetupContext,
    payer: &Keypair,
    payer_path: &Path,
    proof_data_source: ProofDataSource,
) -> Result<()> {
    let language = "typescript-client";
    write_keypair_file(payer, payer_path)
        .map_err(|error| anyhow::anyhow!("write test payer: {error}"))?;
    let ts_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("typescript-client");
    let mut spl_mint = None;
    for name in EXAMPLES {
        eprintln!("Running {language}: {name}");
        let mut command = Command::new(ts_dir.join("node_modules/.bin/tsx"));
        command.arg(ts_dir.join(format!("examples/{name}.ts")));
        let output = command
            .current_dir(&ts_dir)
            .env("ZOLANA_ENDPOINT", &context.rpc_url)
            .env("ZOLANA_INDEXER_URL", &context.indexer_url)
            .env("ZOLANA_PROVER_URL", &context.prover_url)
            .env(
                "ZOLANA_PROOF_DATA_SOURCE",
                match proof_data_source {
                    ProofDataSource::Client => "client",
                    ProofDataSource::Prover => "prover",
                },
            )
            .env("ZOLANA_PAYER_KEYPAIR", payer_path)
            .output()?;
        std::io::stdout().write_all(&output.stdout)?;
        std::io::stderr().write_all(&output.stderr)?;
        ensure!(
            output.status.success(),
            "{language}/{name} failed: {}",
            output.status
        );
        let stdout = String::from_utf8(output.stdout)?;
        if *name == "deposit_with_interface_setup" {
            spl_mint = stdout
                .split_whitespace()
                .find_map(|word| word.strip_prefix("mint="))
                .map(str::to_owned);
            ensure!(
                spl_mint.is_some(),
                "{language} SPL deposit did not report its mint"
            );
        } else if *name == "sync_balance" {
            ensure!(
                stdout.contains("mint="),
                "{language} balance sync returned no balances"
            );
        } else if *name == "read_history" {
            ensure!(
                stdout.contains("kind="),
                "{language} history returned no transactions"
            );
        }
        if matches!(*name, "sync_balance" | "read_history") {
            let mint = spl_mint.as_ref().unwrap();
            ensure!(
                stdout.contains(&format!("mint={mint} ")),
                "{language}/{name} omitted the deposited SPL mint {mint}"
            );
        }
    }
    Ok(())
}

use std::{io::Write, path::Path, process::Command};

use anyhow::{ensure, Result};
use solana_keypair::write_keypair_file;
use zolana_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPorts},
};
use zolana_user_registry_interface::user_registry_program_id;

#[path = "../../test-utils/localnet.rs"]
mod release_localnet;

const EXAMPLES: &[&str] = &[
    "register_private_wallet",
    "deposit_transfer_withdraw",
    "deposit_with_interface_setup",
    "sync_balance",
    "read_history",
];

fn run_examples(language: &str, test_number: u16) -> Result<()> {
    let paths = release_localnet::paths();
    let ports = LocalnetPorts::for_test(test_number)?;
    let _localnet = FixtureLocalnet::start(
        language,
        ports,
        vec![(
            user_registry_program_id(),
            paths.working_dir.join("zolana_user_registry.so"),
        )],
        &paths,
    )?;
    let payer_path = paths.working_dir.join(format!("{language}-payer.json"));
    write_keypair_file(&fixture::payer(), &payer_path)
        .map_err(|error| anyhow::anyhow!("write fixture payer: {error}"))?;
    let rust_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ts_dir = rust_dir.parent().unwrap().join("typescript-client");
    let executable = std::env::current_exe()?;
    let example_dir = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples");

    let mut spl_mint = None;
    for name in EXAMPLES {
        eprintln!("Running {language}: {name}");
        let mut command = if language == "rust-client" {
            Command::new(example_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        } else {
            let mut command = Command::new(ts_dir.join("node_modules/.bin/tsx"));
            command.arg(ts_dir.join(format!("examples/{name}.ts")));
            command
        };
        let output = command
            .current_dir(if language == "rust-client" {
                rust_dir
            } else {
                &ts_dir
            })
            .env("ZOLANA_ENDPOINT", ports.rpc_url())
            .env("ZOLANA_INDEXER_URL", ports.photon_url())
            .env("ZOLANA_PAYER_KEYPAIR", &payer_path)
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

#[test]
fn rust_client_examples_against_release() -> Result<()> {
    run_examples("rust-client", 5)
}

#[test]
fn typescript_client_examples_against_release() -> Result<()> {
    run_examples("typescript-client", 6)
}

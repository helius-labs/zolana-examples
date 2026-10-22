use anyhow::Result;
use rust_client_example::SetupContext;
use zolana_program_test::{
    fixture,
    localnet::{FixtureLocalnet, LocalnetPorts},
};
use zolana_user_registry_interface::user_registry_program_id;

#[path = "../../test-utils/localnet.rs"]
mod release_localnet;
mod shared;

fn start_localnet(language: &str, test_number: u16) -> Result<FixtureLocalnet> {
    let paths = release_localnet::paths();
    FixtureLocalnet::start(
        language,
        LocalnetPorts::for_test(test_number)?,
        vec![(
            user_registry_program_id(),
            paths.working_dir.join("zolana_user_registry.so"),
        )],
        &paths,
    )
    .map_err(Into::into)
}

#[test]
fn rust_client_examples_against_release() -> Result<()> {
    let localnet = start_localnet("rust-client", 5)?;
    shared::run_rust_examples(&localnet.client, localnet.tree, &fixture::payer())
}

#[test]
fn typescript_client_examples_against_release() -> Result<()> {
    let ports = LocalnetPorts::for_test(6)?;
    let localnet = start_localnet("typescript-client", 6)?;
    let context = SetupContext {
        rpc_url: ports.rpc_url(),
        indexer_url: ports.photon_url(),
        prover_url: std::env::var("ZOLANA_PROVER_URL")?,
        tree: localnet.tree,
    };
    shared::run_typescript_examples(
        &context,
        &fixture::payer(),
        &release_localnet::paths()
            .working_dir
            .join("typescript-client-payer.json"),
    )
}

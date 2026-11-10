use spr_program_test::localnet::LocalnetPaths;
use std::{env, path::PathBuf};

/// The release CLI and the deployed shielded-pool program that
/// `test-utils/run-release-tests.sh` exports (`ZOLANA_CLI_BIN` and
/// `SHIELDED_POOL_PROGRAM_PATH`, from `zolana dev artifacts`). The CLI finds the
/// prover, Photon and surfpool through `PROVER_BIN`, `ZOLANA_PHOTON_BIN` and
/// `SURFPOOL_BIN`. Keys and scratch live under `ZOLANA_TEST_ARTIFACTS_DIR`.
pub fn paths() -> LocalnetPaths {
    let dir = env_path("ZOLANA_TEST_ARTIFACTS_DIR");
    LocalnetPaths {
        cli_bin: env_path("ZOLANA_CLI_BIN"),
        shielded_pool_program: env_path("SHIELDED_POOL_PROGRAM_PATH"),
        working_dir: dir.clone(),
        proving_keys_dir: dir.join("proving-keys"),
        scratch_dir: dir.join("localnet"),
    }
}

/// The path in the environment variable `name`, which the release runner
/// exports. Suites that load the deployed user-registry v1.0.0 program read
/// `USER_REGISTRY_PROGRAM_PATH` with it.
pub fn env_path(name: &str) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap_or_else(|| {
        panic!("{name} is not set; run test-utils/run-release-tests.sh to prepare the release artifacts")
    })
}

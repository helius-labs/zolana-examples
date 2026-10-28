use spr_program_test::localnet::LocalnetPaths;
use std::{env, path::PathBuf};

/// Use the verified release artifacts supplied by the integration test runner.
pub fn paths() -> LocalnetPaths {
    let dir = PathBuf::from(
        env::var_os("ZOLANA_TEST_ARTIFACTS_DIR")
            .expect("run test-utils/run-release-tests.sh to prepare the release binaries"),
    );
    LocalnetPaths {
        cli_bin: dir.join("zolana"),
        shielded_pool_program: dir.join("shielded_pool_program.so"),
        working_dir: dir.clone(),
        proving_keys_dir: dir.join("proving-keys"),
        scratch_dir: dir.join("localnet"),
    }
}

#!/usr/bin/env bash
set -euo pipefail

repo_dir=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo_dir"
export ZOLANA_TEST_ARTIFACTS_DIR="${ZOLANA_TEST_ARTIFACTS_DIR:-$repo_dir/target/release-tests}"
mkdir -p "$ZOLANA_TEST_ARTIFACTS_DIR"
ZOLANA_TEST_ARTIFACTS_DIR=$(cd "$ZOLANA_TEST_ARTIFACTS_DIR" && pwd)
export ZOLANA_TEST_ARTIFACTS_DIR
python3 test-utils/download-release.py "$ZOLANA_TEST_ARTIFACTS_DIR"

export SURFPOOL_BIN="$ZOLANA_TEST_ARTIFACTS_DIR/surfpool"
export ZOLANA_PHOTON_BIN="$ZOLANA_TEST_ARTIFACTS_DIR/photon"
export ZOLANA_PROVER_BIN="$ZOLANA_TEST_ARTIFACTS_DIR/prover-server"
export ZOLANA_PROVER_AUTO_DOWNLOAD=true
export ZOLANA_PROVER_URL=http://127.0.0.1:13301
export ZOLANA_PORT_OFFSET=500
export ZOLANA_PROCESS_SCOPE_DIR="$ZOLANA_TEST_ARTIFACTS_DIR/processes"
mkdir -p "$ZOLANA_PROCESS_SCOPE_DIR" "$ZOLANA_TEST_ARTIFACTS_DIR/proving-keys"
python3 - <<'PY'
import socket
for port in (13301, 20298):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", port))
PY
"$SURFPOOL_BIN" --version

cleanup() {
    for test_number in 1 2 3 4 5 6; do
        "$ZOLANA_TEST_ARTIFACTS_DIR/zolana" test-env --stop \
            --rpc-port "$((8899 + 500 + 1000 * test_number))" \
            --photon-port "$((8784 + 500 + 1000 * test_number))" \
            --prover-port 13301 || true
    done
}
trap cleanup EXIT

# SBF builds keep their default output directories; host builds share one cache.
CARGO_TARGET_DIR="$repo_dir/swap-program/program/target" cargo build-sbf \
    --manifest-path swap-program/program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint
CARGO_TARGET_DIR="$repo_dir/escrow-program/target" cargo build-sbf \
    --manifest-path escrow-program/program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_dir/target/integration}"

for circuit in make take cancel take_verifiable_encryption; do
    cargo run --manifest-path swap-program/prover/Cargo.toml --bin swap-prover-setup -- \
        "$circuit" "$repo_dir/swap-program/build/gnark/$circuit" --insecure-test-keys
done
for circuit in escrow withdraw; do
    cargo run --manifest-path escrow-program/prover/Cargo.toml --bin timelock-escrow-prover-setup -- \
        "$circuit" "$repo_dir/escrow-program/build/gnark/$circuit" --insecure-test-keys
done

# Each suite starts its own validator and indexer and shares the pinned prover.
cargo test --manifest-path swap-program/test/Cargo.toml \
    --test swap --test cancel --test take_verifiable_encryption -- --nocapture --test-threads=1
cargo test --manifest-path escrow-program/Cargo.toml -p timelock-escrow-test \
    --test escrow -- --nocapture --test-threads=1
cargo test --manifest-path rust-client/Cargo.toml --test client_localnet --locked \
    -- --nocapture --test-threads=1

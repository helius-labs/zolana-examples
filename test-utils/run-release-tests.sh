#!/usr/bin/env bash
# Run the gnark swap and escrow localnet tests and the rust-client localnet
# tests against the Zolana cli-v0.10.0 release: its CLI, prover, Photon and
# surfpool, the deployed shielded-pool v1.0.0 program and the deployed
# user-registry v1.0.0 program. Nothing here clones or builds Zolana.
#
# ZOLANA_PORT_OFFSET (default 500) shifts the localnet ports and
# ZOLANA_PROVER_URL (default http://127.0.0.1:13301) picks the prover port, so a
# run can avoid ports another checkout uses.
set -euo pipefail

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=zolana-release.sh
source "$repo_dir/test-utils/zolana-release.sh"
cd "$repo_dir"
export ZOLANA_TEST_ARTIFACTS_DIR="${ZOLANA_TEST_ARTIFACTS_DIR:-$repo_dir/target/release-tests}"
mkdir -p "$ZOLANA_TEST_ARTIFACTS_DIR"
ZOLANA_TEST_ARTIFACTS_DIR=$(cd "$ZOLANA_TEST_ARTIFACTS_DIR" && pwd)
# The CLI caches the artifacts it downloads under $ZOLANA_CONFIG_DIR/cache.
export ZOLANA_CONFIG_DIR="$ZOLANA_TEST_ARTIFACTS_DIR/zolana-config"

# 1. The release CLI, checked against the release's SHA256SUMS.
release_dir="$ZOLANA_TEST_ARTIFACTS_DIR/$zolana_release"
cli="$ZOLANA_TEST_ARTIFACTS_DIR/bin/zolana"
install_zolana_cli "$release_dir" "$cli"

# 2. The prover, Photon, surfpool and shielded-pool program the CLI pins. It
# verifies each download and prints ZOLANA_CLI_BIN, SHIELDED_POOL_PROGRAM_PATH,
# PROVER_BIN, ZOLANA_PHOTON_BIN and SURFPOOL_BIN as NAME=path lines.
artifacts=$("$cli" dev artifacts)
while IFS='=' read -r name value; do
    case $name in
        ZOLANA_CLI_BIN | SHIELDED_POOL_PROGRAM_PATH | PROVER_BIN | ZOLANA_PHOTON_BIN | SURFPOOL_BIN)
            export "$name=$value"
            ;;
        *)
            echo "Unexpected zolana dev artifacts line: $name=$value" >&2
            exit 1
            ;;
    esac
done <<<"$artifacts"
for name in ZOLANA_CLI_BIN SHIELDED_POOL_PROGRAM_PATH PROVER_BIN ZOLANA_PHOTON_BIN SURFPOOL_BIN; do
    if [[ ! -f ${!name:-} ]]; then
        echo "zolana dev artifacts did not provide $name" >&2
        exit 1
    fi
done

# 3. The deployed user registry.
export USER_REGISTRY_PROGRAM_PATH="$ZOLANA_TEST_ARTIFACTS_DIR/$user_registry_release/spr_user_registry.so"
fetch_user_registry "$USER_REGISTRY_PROGRAM_PATH"

export ZOLANA_PROVER_AUTO_DOWNLOAD=true
export ZOLANA_PROVER_URL="${ZOLANA_PROVER_URL:-http://127.0.0.1:13301}"
export ZOLANA_PORT_OFFSET="${ZOLANA_PORT_OFFSET:-500}"
export ZOLANA_PROCESS_SCOPE_DIR="$ZOLANA_TEST_ARTIFACTS_DIR/processes"
mkdir -p "$ZOLANA_PROCESS_SCOPE_DIR" "$ZOLANA_TEST_ARTIFACTS_DIR/proving-keys"
prover_port=${ZOLANA_PROVER_URL##*:}
prover_port=${prover_port%/}
# The CLI puts the prover's metrics port at 9998 plus the prover port's offset
# from 3001.
python3 - "$prover_port" "$((9998 + prover_port - 3001))" <<'PY'
import socket
import sys
for port in sys.argv[1:]:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", int(port)))
PY
"$SURFPOOL_BIN" --version

# The suites use test numbers 1 to 6 (LocalnetPorts::for_test).
cleanup() {
    for test_number in 1 2 3 4 5 6; do
        "$ZOLANA_CLI_BIN" test-env --stop \
            --rpc-port "$((8899 + ZOLANA_PORT_OFFSET + 1000 * test_number))" \
            --photon-port "$((8784 + ZOLANA_PORT_OFFSET + 1000 * test_number))" \
            --prover-port "$prover_port" || true
    done
}
trap cleanup EXIT

# SBF builds keep their default output directories; host builds share one cache.
CARGO_TARGET_DIR="$repo_dir/swap/gnark/program/target" cargo build-sbf \
    --manifest-path swap/gnark/program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint
CARGO_TARGET_DIR="$repo_dir/escrow/gnark/target" cargo build-sbf \
    --manifest-path escrow/gnark/program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_dir/target/integration}"

for circuit in make take cancel; do
    cargo run --manifest-path swap/gnark/prover/Cargo.toml --bin swap-prover-setup -- \
        "$circuit" "$repo_dir/swap/gnark/build/gnark/$circuit" --insecure-test-keys
done
for circuit in escrow withdraw; do
    cargo run --manifest-path escrow/gnark/prover/Cargo.toml --bin timelock-escrow-prover-setup -- \
        "$circuit" "$repo_dir/escrow/gnark/build/gnark/$circuit" --insecure-test-keys
done
# The suites must prove with the pinned insecure test keys.
swap/gnark/scripts/check-keys.sh
escrow/gnark/scripts/check-keys.sh

# Each suite starts its own validator and indexer and shares the release prover.
cargo test --manifest-path swap/gnark/test/Cargo.toml \
    --test swap --test cancel -- --nocapture --test-threads=1
cargo test --manifest-path escrow/gnark/Cargo.toml -p timelock-escrow-test \
    --test escrow -- --nocapture --test-threads=1
cargo test --manifest-path rust-client/Cargo.toml --test client_localnet --locked \
    -- --nocapture --test-threads=1

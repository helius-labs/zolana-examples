#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
source_root="$root/.cache/zolana"
revision=ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856
export PATH="$root/.cache/agave/solana-release/bin:$PATH"
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_TARGET_DIR="$root/target/program-examples"
export ZOLANA_CLI_BIN="$source_root/target/debug/zolana"
export ZOLANA_PHOTON_BIN="$source_root/target/debug/photon"
export SURFPOOL_BIN="$source_root/target/tools/surfpool"
export ZOLANA_PROVER_BIN="$source_root/target/prover-server"
export ZOLANA_PROVER_KEYS_DIR="$source_root/prover/server/proving-keys"
export ZOLANA_LOCALNET_RPC_PORT=8899
export ZOLANA_LOCALNET_PHOTON_PORT=8784
export ZOLANA_LOCALNET_URL=http://127.0.0.1:8899
export ZOLANA_INDEXER_URL=http://127.0.0.1:8784
export ZOLANA_PROVER_URL=http://127.0.0.1:3001
cd "$root"

require_source() {
    if [[ ! -d "$source_root/.git" ]] || [[ $(git -C "$source_root" rev-parse HEAD) != "$revision" ]]; then
        echo "Run scripts/program-examples.sh prepare to create the pinned Zolana checkout." >&2
        exit 1
    fi
    if [[ -n $(git -C "$source_root" status --porcelain --untracked-files=no) ]]; then
        echo "The cached Zolana checkout has tracked changes; use a clean checkout at $revision." >&2
        exit 1
    fi
}

build_sbf() {
    cargo +1.98.1 build-sbf --tools-version v1.54 --sbf-out-dir "$2" \
        --manifest-path "$1" -- --locked --features "$3"
}

prepare() {
    for tool in git cargo go just gh clang lsof; do
        command -v "$tool" >/dev/null || { echo "Install $tool before preparing the examples." >&2; exit 1; }
    done
    if [[ ! -e "$source_root" ]]; then
        git clone --no-checkout https://github.com/helius-labs/zolana "$source_root"
        git -C "$source_root" checkout --detach "$revision"
    fi
    require_source
    if [[ ! -x "$root/.cache/agave/solana-release/bin/solana-test-validator" ]]; then
        case "$(uname -s)-$(uname -m)" in
            Darwin-arm64) platform=aarch64-apple-darwin ;;
            Darwin-x86_64) platform=x86_64-apple-darwin ;;
            Linux-x86_64) platform=x86_64-unknown-linux-gnu ;;
            *) echo "Unsupported Agave platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
        esac
        mkdir -p "$root/.cache/agave"
        gh release download v4.2.2 --repo anza-xyz/agave \
            --pattern "solana-release-$platform.tar.bz2" --dir "$root/.cache/agave" --clobber
        tar -xjf "$root/.cache/agave/solana-release-$platform.tar.bz2" -C "$root/.cache/agave"
    fi
    (
        cd "$source_root"
        export CARGO_TARGET_DIR="$source_root/target"
        cargo +1.98.1 build --locked -p zolana-cli -p xtask -p photon-indexer --bin zolana --bin xtask --bin photon
        just build-prover-server ensure-smart-account
        if [[ ! -x "$SURFPOOL_BIN" ]]; then just install-surfpool; fi
    )
    build_sbf "$source_root/programs/shielded-pool/Cargo.toml" "$source_root/target/deploy" bpf-entrypoint
    build_sbf "$source_root/programs/user-registry/Cargo.toml" "$source_root/target/deploy" bpf-entrypoint
    build_sbf "$root/compression-program/program/Cargo.toml" "$source_root/target/deploy" bpf-entrypoint
    build_sbf "$root/dynamic-swap-program/program/Cargo.toml" "$source_root/target/deploy" bpf-entrypoint
    (
        cd "$source_root"
        just ensure-dynamic-swap-keys
    )
    mkdir -p "$root/dynamic-swap-program/build/gnark"
    cp -R "$source_root/sdk-tests/dynamic-swap/build/gnark/escrow_open" "$root/dynamic-swap-program/build/gnark/"
    cp -R "$source_root/sdk-tests/dynamic-swap/build/gnark/escrow_settle" "$root/dynamic-swap-program/build/gnark/"
}

check() {
    require_source
    for dir in compression-program dynamic-swap-program rfq; do
        cargo +1.98.1 fmt --manifest-path "$root/$dir/Cargo.toml" --all -- --check
        cargo +1.98.1 check --manifest-path "$root/$dir/Cargo.toml" --workspace --all-targets --locked
    done
}

# The upstream CLI restarts validator processes by name. Refuse to disturb an
# existing stack; after a test, stop only services launched from this cache.
require_idle() {
    if pgrep -f '(^|/)(solana-test-validator|surfpool|photon|prover-server)( |$)' >/dev/null; then
        echo "Stop the existing local validator, Photon, and prover before running these tests." >&2
        exit 1
    fi
    for port in 8899 8900 8784 3001 9998; do
        if lsof -tiTCP:"$port" -sTCP:LISTEN >/dev/null; then
            echo "Port $port is in use; stop that service before running these tests." >&2
            exit 1
        fi
    done
}

cleanup() {
    while read -r pid command; do
        case "$command" in
            *"$source_root/"*surfpool*|*"$source_root/"*photon*|*"$source_root/"*prover-server*|*"$root/.cache/agave/solana-release/bin/solana-test-validator"*)
                kill "$pid" 2>/dev/null || true
                for _ in {1..30}; do
                    kill -0 "$pid" 2>/dev/null || break
                    sleep 0.1
                done
                if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid" 2>/dev/null || true; fi
                ;;
        esac
    done < <(ps -axo pid=,command=)
}

run_test() {
    require_idle
    trap cleanup EXIT
    case "$1" in
        compression) cargo +1.98.1 test --manifest-path compression-program/Cargo.toml --workspace --locked -- --nocapture --test-threads=1 ;;
        dynamic-swap) cargo +1.98.1 test --manifest-path dynamic-swap-program/Cargo.toml --workspace --locked -- --nocapture --test-threads=1 ;;
        rfq) cargo +1.98.1 test --manifest-path rfq/Cargo.toml --locked -- --nocapture --test-threads=1 ;;
        *) echo "Unknown example: $1" >&2; exit 1 ;;
    esac
    cleanup
    trap - EXIT
}

bench() {
    require_idle
    trap cleanup EXIT
    case "$1" in
        dynamic-swap)
            out="$source_root/target/dynamic-swap-bench"
            build_sbf "$source_root/programs/shielded-pool/Cargo.toml" "$out" bpf-entrypoint
            build_sbf "$root/dynamic-swap-program/program/Cargo.toml" "$out" bpf-entrypoint,profile-program
            cargo +1.98.1 test --manifest-path dynamic-swap-program/Cargo.toml -p dynamic-swap-test --locked --test bench_cu -- --ignored --nocapture
            ;;
        rfq)
            out="$source_root/target/rfq-bench"
            build_sbf "$source_root/programs/shielded-pool/Cargo.toml" "$out" bpf-entrypoint,profile-program
            cargo +1.98.1 test --manifest-path rfq/Cargo.toml --locked --test bench_cu -- --ignored --nocapture
            ;;
        *) echo "Benchmarks: dynamic-swap or rfq" >&2; exit 1 ;;
    esac
    cleanup
    trap - EXIT
}

case "${1:-}" in
    prepare) prepare ;;
    check) check ;;
    test)
        require_source
        if [[ ${2:-all} == all ]]; then
            for example in compression dynamic-swap rfq; do run_test "$example"; done
        else
            run_test "$2"
        fi
        ;;
    bench) require_source; bench "${2:-}" ;;
    *) echo "Usage: $0 prepare|check|test [compression|dynamic-swap|rfq|all]|bench [dynamic-swap|rfq]" >&2; exit 1 ;;
esac

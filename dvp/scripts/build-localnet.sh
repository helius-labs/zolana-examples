#!/usr/bin/env bash
# Build the localnet prover, Photon and SPP program from the pinned SDK
# checkout. The SDK revision is newer than its last protocol release: the
# release prover lacks the proving-key artifacts the SDK checks, and the
# release program rejects proofs from the SDK revision's prover keys.
set -euo pipefail
# Build output goes to stderr; stdout carries only the variables to export.
exec 3>&1 1>&2

dvp=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana="$dvp/.tools/zolana"
sbf_tools_version="${SBF_TOOLS_VERSION:-v1.54}"

mkdir -p "$zolana/target/deploy"
(cd "$zolana/prover/server" && sh build-release.sh ../../target/prover-server)
cargo build --release --locked --manifest-path "$zolana/Cargo.toml" -p photon-indexer --bin photon
cargo build-sbf --tools-version "$sbf_tools_version" --sbf-out-dir "$zolana/target/deploy" \
    --manifest-path "$zolana/programs/shielded-pool/Cargo.toml" -- --locked --features bpf-entrypoint

{
    echo "PROVER_BIN=$zolana/target/prover-server"
    echo "ZOLANA_PHOTON_BIN=$dvp/scripts/photon-localnet.sh"
    echo "DVP_PHOTON_BIN=$zolana/target/release/photon"
    echo "SHIELDED_POOL_PROGRAM_PATH=$zolana/target/deploy/shielded_pool_program.so"
} >&3

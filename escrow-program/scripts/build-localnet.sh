#!/usr/bin/env bash
set -euo pipefail

base="$(cd "$(dirname "$0")/.." && pwd)"
"$base/scripts/prepare-zolana.sh"

cd "$base/target/zolana"
if [[ ! -x target/tools/surfpool ]]; then
    just install-surfpool
fi
cargo +1.97.0 build --locked -p zolana-cli -p photon-indexer --bin zolana --bin photon
(cd prover/server && go build -o ../../target/prover-server .)
cargo +1.97.0 build-sbf --tools-version v1.54 \
    --manifest-path programs/shielded-pool/Cargo.toml \
    --sbf-out-dir target/deploy -- --locked --features bpf-entrypoint
if [[ ! -f target/deploy/squads_smart_account_program.so ]]; then
    solana program dump SMRTzfY6DfH5ik3TKiyLFfXexV8uSG3d2UksSCYdunG \
        target/deploy/squads_smart_account_program.so --url https://api.mainnet-beta.solana.com
fi

cd "$base"
cargo +1.97.0 build-sbf --tools-version v1.54 \
    --manifest-path program/Cargo.toml --sbf-out-dir target/deploy \
    -- --locked --features bpf-entrypoint

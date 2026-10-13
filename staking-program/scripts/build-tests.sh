#!/usr/bin/env bash
set -euo pipefail

staking_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
sbf_tools_version="${SBF_TOOLS_VERSION:-v1.54}"
bash "$staking_root/scripts/prepare-deps.sh"

cargo build --manifest-path "$staking_root/.deps/zolana/Cargo.toml" -p zolana-cli --locked
(
  cd "$staking_root/.deps/zolana/prover/server"
  go build -o "$staking_root/.deps/zolana/target/debug/zolana-prover" .
)
(
  cd "$staking_root/.deps/zolana/programs/shielded-pool"
  cargo build-sbf --tools-version "$sbf_tools_version" -- --features bpf-entrypoint --locked
)
(
  cd "$staking_root/program"
  cargo build-sbf --tools-version "$sbf_tools_version" -- --features bpf-entrypoint --locked
)

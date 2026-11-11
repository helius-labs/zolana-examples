#!/usr/bin/env bash
# Start the cli-v0.10.0 release localnet for the smart-account-client tests:
# surfpool with the shielded pool, the user registry and the Squads
# smart-account program the release pins, plus the prover and Photon. Adds the
# data registry program (program/), built here, and the Squads ProgramConfig
# account the release snapshot leaves out.
#
#   bash scripts/localnet.sh          # start
#   bash scripts/localnet.sh --stop   # stop
#
# Ports: RPC 18899, Photon 18784, prover 13301 (override with SA_RPC_PORT,
# SA_PHOTON_PORT, SA_PROVER_PORT). Everything lives under target/sa-client.
set -euo pipefail

client_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo_dir=$(cd "$client_dir/.." && pwd)
# shellcheck source=../../test-utils/zolana-release.sh
source "$repo_dir/test-utils/zolana-release.sh"

work_dir="$repo_dir/target/sa-client"
cli="$work_dir/bin/zolana"
export ZOLANA_CONFIG_DIR="$work_dir/zolana-config"
rpc_port=${SA_RPC_PORT:-18899}
photon_port=${SA_PHOTON_PORT:-18784}
prover_port=${SA_PROVER_PORT:-13301}
ports=(--rpc-port "$rpc_port" --photon-port "$photon_port" --prover-port "$prover_port")

if [[ ${1:-} == "--stop" ]]; then
    "$cli" dev start --stop "${ports[@]}"
    exit 0
fi

mkdir -p "$work_dir/logs"
if [[ ! -x $cli ]]; then
    install_zolana_cli "$work_dir/$zolana_release" "$cli"
fi

# The data registry keeps the vault's key record. `--sbf-program` would turn
# off the release artifacts, so it goes to surfpool as a passthrough argument.
data_registry_program=Eoxe4CcAxAjM4FE4YcR889kDVDmZvr9txS74LQ8ybHdU
cargo build-sbf --tools-version "${SBF_TOOLS_VERSION:-v1.54}" \
    --manifest-path "$client_dir/program/Cargo.toml" \
    -- --locked --features bpf-entrypoint
data_registry_so="$client_dir/program/target/deploy/data_registry.so"

account_dir="$work_dir/accounts"
rm -rf "$account_dir"
(cd "$client_dir" && node --import tsx scripts/program-config.ts "$account_dir")

"$cli" dev start "${ports[@]}" \
    --slot-time 100 \
    --account-dir "$account_dir" \
    --log-dir "$work_dir/logs" \
    -- --bpf-program "$data_registry_program" "$data_registry_so"

cat <<EOF
Localnet ready. Run the tests with:
  SA_RPC_URL=http://127.0.0.1:$rpc_port SA_INDEXER_URL=http://127.0.0.1:$photon_port \\
  SA_PROVER_URL=http://127.0.0.1:$prover_port pnpm test:localnet
EOF

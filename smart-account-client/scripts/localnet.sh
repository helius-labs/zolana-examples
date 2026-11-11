#!/usr/bin/env bash
# Start the cli-v0.10.0 release localnet for the smart-account-client tests:
# surfpool with the shielded pool and the Squads smart-account program the
# release pins, plus the prover and Photon. Replaces the release user registry
# with one built from the pinned privacy-rings commit, which has set_data,
# and adds the Squads ProgramConfig account the release snapshot leaves out.
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

# The user registry with set_data comes from the privacy-rings commit the
# @heliuslabs/spr dependency pins, built here. `--sbf-program` would turn off
# the release artifacts, so it goes to surfpool as a passthrough argument
# after the release programs, and the later load replaces the release one.
registry_revision=$(cd "$client_dir" && node -p \
    'require("./package.json").dependencies["@heliuslabs/spr"].match(/#([0-9a-f]{40})/)[1]')
registry_program=regy3WAeRXMYuMYxr5iK3Bf9Arr3PcxGxyzAddbiqDQ
registry_so="$work_dir/registry/$registry_revision/spr_user_registry.so"
if [[ ! -f $registry_so ]]; then
    source_dir="$work_dir/registry-src"
    rm -rf "$source_dir"
    git init -q "$source_dir"
    git -C "$source_dir" fetch -q --depth 1 \
        https://github.com/helius-labs/privacy-rings.git "$registry_revision"
    git -C "$source_dir" checkout -q FETCH_HEAD
    # The flags of the repository's tools/build-programs.sh.
    cargo build-sbf --tools-version "${SBF_TOOLS_VERSION:-v1.54}" \
        --sbf-out-dir "$(dirname "$registry_so")" \
        --manifest-path "$source_dir/programs/user-registry/Cargo.toml" \
        -- --locked --features bpf-entrypoint
fi

account_dir="$work_dir/accounts"
rm -rf "$account_dir"
(cd "$client_dir" && node --import tsx scripts/program-config.ts "$account_dir")

"$cli" dev start "${ports[@]}" \
    --slot-time 100 \
    --account-dir "$account_dir" \
    --log-dir "$work_dir/logs" \
    -- --bpf-program "$registry_program" "$registry_so"

cat <<EOF
Localnet ready. Run the tests with:
  SA_RPC_URL=http://127.0.0.1:$rpc_port SA_INDEXER_URL=http://127.0.0.1:$photon_port \\
  SA_PROVER_URL=http://127.0.0.1:$prover_port pnpm test:localnet
EOF

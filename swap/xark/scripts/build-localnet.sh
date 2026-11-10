#!/usr/bin/env bash
# Download the localnet prover, Photon, surfpool and shielded-pool program that
# the Zolana release CLI pins, plus the user-registry release program, and
# print the variables to export on stdout.
set -euo pipefail

swap=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../../../test-utils/zolana-release.sh
source "$swap/../../test-utils/zolana-release.sh"
zolana_cli="${ZOLANA_CLI_BIN:-$swap/.tools/bin/zolana}"
if [[ ! -x $zolana_cli ]]; then
    echo "No zolana CLI at $zolana_cli; run scripts/setup-zk-tools.sh first" >&2
    exit 1
fi

# Prints ZOLANA_CLI_BIN, SHIELDED_POOL_PROGRAM_PATH, PROVER_BIN,
# ZOLANA_PHOTON_BIN and SURFPOOL_BIN; progress goes to stderr.
"$zolana_cli" dev artifacts

# The maker registers its shielded address and the taker resolves it there.
# `dev artifacts` does not include the user registry, so fetch the deployed
# release program, which matches the pinned spr-user-registry-interface.
user_registry="$swap/.tools/$user_registry_release/spr_user_registry.so"
fetch_user_registry "$user_registry"
echo "USER_REGISTRY_PROGRAM_PATH=$user_registry"

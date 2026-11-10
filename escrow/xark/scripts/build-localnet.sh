#!/usr/bin/env bash
# Download the localnet prover, Photon, surfpool and shielded-pool program that
# the Zolana release CLI pins, and print the variables to export on stdout.
set -euo pipefail

escrow=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana_cli="${ZOLANA_CLI_BIN:-$escrow/.tools/bin/zolana}"
if [[ ! -x $zolana_cli ]]; then
    echo "No zolana CLI at $zolana_cli; run scripts/setup-zk-tools.sh first" >&2
    exit 1
fi

# Prints ZOLANA_CLI_BIN, SHIELDED_POOL_PROGRAM_PATH, PROVER_BIN,
# ZOLANA_PHOTON_BIN and SURFPOOL_BIN; progress goes to stderr.
"$zolana_cli" dev artifacts

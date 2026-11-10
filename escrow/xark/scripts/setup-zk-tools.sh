#!/usr/bin/env bash
# Install the Zolana release CLI that matches the pinned SDK crates and print
# ZOLANA_CLI_BIN. The CLI bundles Xark; `zk compile` and check-keys.sh verify
# its output. --check-pins only checks the Zolana Git pins in Cargo.toml.
set -euo pipefail

escrow=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../../../test-utils/zolana-release.sh
source "$escrow/../../test-utils/zolana-release.sh"

check_pins "$escrow/Cargo.toml"
if [[ ${1:-} == --check-pins ]]; then
    exit 0
fi

install_zolana_cli "$escrow/.tools/$zolana_release" "$escrow/.tools/bin/zolana"
echo "ZOLANA_CLI_BIN=$escrow/.tools/bin/zolana"

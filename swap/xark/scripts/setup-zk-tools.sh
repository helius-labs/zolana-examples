#!/usr/bin/env bash
# Install the Zolana release CLI that matches the pinned SDK crates and print
# ZOLANA_CLI_BIN. The CLI bundles Xark; `zk compile` and check-keys.sh verify
# its output. --check-pins only checks the Zolana Git pins in Cargo.toml.
set -euo pipefail

swap=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../../../test-utils/zolana-release.sh
source "$swap/../../test-utils/zolana-release.sh"

check_pins "$swap/Cargo.toml"
if [[ ${1:-} == --check-pins ]]; then
    exit 0
fi

install_zolana_cli "$swap/.tools/$zolana_release" "$swap/.tools/bin/zolana"
echo "ZOLANA_CLI_BIN=$swap/.tools/bin/zolana"

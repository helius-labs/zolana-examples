#!/usr/bin/env bash
# Check generated Xark bytecode and keys against trusted checksums.
set -euo pipefail

swap="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${SWAP_KEYS_DIR:-$swap/target/zk/swap-program}"
shasum -a 256 --check --strict "$swap/swap-keys.CHECKSUM"

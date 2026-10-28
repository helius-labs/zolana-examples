#!/usr/bin/env bash
# Check generated Xark bytecode and keys against trusted checksums.
set -euo pipefail

dvp="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${DVP_KEYS_DIR:-$dvp/target/zk/dvp-program}"
shasum -a 256 --check --strict "$dvp/dvp-keys.CHECKSUM"

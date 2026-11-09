#!/usr/bin/env bash
# Check generated Xark bytecode and keys against trusted checksums.
set -euo pipefail

escrow="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${TIMELOCK_ESCROW_KEYS_DIR:-$escrow/target/zk/timelock-escrow-program}"
shasum -a 256 --check --strict "$escrow/timelock-escrow-keys.CHECKSUM"

#!/usr/bin/env bash
# Check the insecure test keys in build/gnark against timelock-escrow-keys.CHECKSUM, which
# names each key <circuit>_<pk|vk>.bin.
set -euo pipefail

example="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
keys="$(mktemp -d)"
trap 'rm -rf "$keys"' EXIT
for circuit in escrow withdraw; do
    for key in pk vk; do
        ln -s "$example/build/gnark/$circuit/$key.bin" "$keys/${circuit}_$key.bin"
    done
done
cd "$keys"
shasum -a 256 --check --strict "$example/timelock-escrow-keys.CHECKSUM"

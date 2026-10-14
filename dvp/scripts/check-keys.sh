#!/usr/bin/env bash
# Checks the circuit keys in build/gnark against dvp-keys.CHECKSUM:
#   dvp/scripts/check-keys.sh
#
# The checksum file lists `<sha256>  <circuit>_<pk|vk>.bin` for each circuit's
# build/gnark/<circuit>/{pk,vk}.bin, sorted by name. Fails with a diff when a
# key is missing or its hash differs.
set -euo pipefail

dvp="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$dvp"

for c in create_dvp reclaim_dvp settle_dvp abort_dvp; do
  for k in pk vk; do
    hash=$(shasum -a 256 <"build/gnark/$c/$k.bin" | cut -d' ' -f1)
    printf '%s  %s\n' "$hash" "${c}_${k}.bin"
  done
done | LC_ALL=C sort -k2 | diff -u dvp-keys.CHECKSUM -

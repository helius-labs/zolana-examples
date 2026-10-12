#!/usr/bin/env bash
# Download the SPP proving keys the tests use into staking-program/build/spp-keys
# ahead of the prover, so CI can cache them. The prover downloads missing keys
# itself; this only avoids repeating that per run. Keys are verified against the
# pinned lock file. Set ZOLANA_PROVING_KEYS_URL to use a mirror.
set -euo pipefail

staking_root="$(cd "$(dirname "$0")/.." && pwd)"
lock="$staking_root/.deps/zolana/prover/server/prover/provingkeys/proving-keys.lock"
base="${ZOLANA_PROVING_KEYS_URL:-https://d3gbdb0egjwcw9.cloudfront.net}"
prefix="$(jq -r .prefix "$lock")"
out_dir="$staking_root/build/spp-keys"
mkdir -p "$out_dir"
for name in transfer_confidential_2_2 transfer_confidential_16_2 transfer_confidential_1_16; do
    file="$name.key"
    expected="$(jq -r ".keys[\"$file\"].sha256" "$lock")"
    out="$out_dir/$file"
    if [[ -f "$out" ]] && [[ "$(shasum -a 256 "$out" | cut -d' ' -f1)" == "$expected" ]]; then
        continue
    fi
    curl -sSfL "$base/$prefix/$file" -o "$out.tmp"
    actual="$(shasum -a 256 "$out.tmp" | cut -d' ' -f1)"
    if [[ "$actual" != "$expected" ]]; then
        rm -f "$out.tmp"
        echo "checksum mismatch for $file" >&2
        exit 1
    fi
    mv "$out.tmp" "$out"
done

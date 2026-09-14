#!/usr/bin/env bash
set -euo pipefail

base="$(cd "$(dirname "$0")/.." && pwd)"
revision=af11be0e8d27702f8e6553320bd5dab52ab79fed
checkout="$base/target/zolana"

if [[ ! -d "$checkout/.git" ]]; then
    mkdir -p "$base/target"
    git clone --no-checkout https://github.com/helius-labs/zolana "$checkout"
    git -C "$checkout" checkout --detach "$revision"
fi
[[ "$(git -C "$checkout" rev-parse HEAD)" == "$revision" ]] || {
    echo "Unexpected Zolana revision in $checkout; expected $revision" >&2
    exit 1
}
git -C "$checkout" diff --quiet HEAD -- || {
    echo "Zolana source checkout has changes: $checkout" >&2
    exit 1
}

for circuit in escrow withdraw; do
    dir="$base/build/gnark/$circuit"
    mkdir -p "$dir"
    for kind in pk vk; do
        asset="${circuit}_${kind}.bin"
        want="$(awk -v name="$asset" '$2 == name { print $1 }' "$base/timelock-escrow-keys.CHECKSUM")"
        [[ -n "$want" ]] || { echo "Missing checksum for $asset" >&2; exit 1; }
        if [[ ! -f "$dir/$kind.bin" ]]; then
            temporary="$(mktemp "$dir/download.XXXXXX")"
            trap 'rm -f "$temporary"' EXIT
            curl --fail --location --output "$temporary" \
                "https://github.com/helius-labs/zolana/releases/download/escrow-keys-v6/$asset"
            [[ "$(shasum -a 256 "$temporary" | awk '{print $1}')" == "$want" ]] || {
                echo "Downloaded key checksum mismatch: $asset" >&2
                exit 1
            }
            mv "$temporary" "$dir/$kind.bin"
            trap - EXIT
        fi
        [[ "$(shasum -a 256 "$dir/$kind.bin" | awk '{print $1}')" == "$want" ]] || {
            echo "Existing key checksum mismatch: $dir/$kind.bin" >&2
            exit 1
        }
    done
done
echo "Prepared Zolana $revision and verified escrow-keys-v6"

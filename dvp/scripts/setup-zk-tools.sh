#!/usr/bin/env bash
# Prepare the exact SDK source used by this example and build its tools.
set -euo pipefail

dvp=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana_revision=c32e1ca72fa38beacdfd250a47abb214b85202a7
xark_revision=ad7baad990cb949227ad7d6046e4cfbe3806d357

checkout() {
    local repository=$1 revision=$2 directory=$3
    if [[ ! -e "$directory/.git" ]]; then
        mkdir -p "$directory"
        git -C "$directory" init --quiet
        git -C "$directory" remote add origin "$repository"
        git -C "$directory" fetch --depth 1 origin "$revision"
        git -C "$directory" checkout --quiet --detach FETCH_HEAD
    fi
    if [[ $(git -C "$directory" rev-parse HEAD) != "$revision" ]]; then
        echo "Expected $directory at $revision" >&2
        exit 1
    fi
}

# Cargo manifests cannot read these variables; fail when any of their Git
# pins drifts from the revisions above.
check_pins() {
    local repository=$1 revision=$2 file
    shift 2
    for file in "$@"; do
        if grep -F "$repository" "$file" | grep -F 'rev = ' | grep -vqF "$revision"; then
            echo "$file pins $repository to a revision other than $revision" >&2
            exit 1
        fi
    done
}

checkout https://github.com/helius-labs/zolana.git "$zolana_revision" "$dvp/.tools/zolana"
check_pins https://github.com/helius-labs/zolana "$zolana_revision" "$dvp/Cargo.toml"
# The SDK, its CLI and the generated circuit crates must share one Xark.
check_pins https://github.com/ananas-block/xark "$xark_revision" \
    "$dvp/Cargo.toml" "$dvp/.tools/zolana/Cargo.toml"

if [[ ${1:-} == --prepare-only ]]; then
    exit 0
fi

cargo build --release --manifest-path "$dvp/.tools/zolana/Cargo.toml" -p spr-cli -p xark-cli
echo "Zolana CLI: $dvp/.tools/zolana/target/release/zolana"
echo "Xark CLI: $dvp/.tools/zolana/target/release/xark"

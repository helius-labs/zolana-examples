#!/usr/bin/env bash
# Prepare the exact SDK source used by this example and build its tools.
set -euo pipefail

swap=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana_revision=af9d32923c047e17c7a5b2985c92492ae18bfa2e
xark_revision=aec8530d4526a082daed68ca2dd9c21d47a86f26

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

checkout https://github.com/helius-labs/zolana.git "$zolana_revision" "$swap/.tools/zolana"
check_pins https://github.com/helius-labs/zolana "$zolana_revision" "$swap/Cargo.toml"
# The SDK, its CLI and the generated circuit crates must share one Xark.
check_pins https://github.com/ananas-block/xark "$xark_revision" \
    "$swap/Cargo.toml" "$swap/.tools/zolana/Cargo.toml"

if [[ ${1:-} == --prepare-only ]]; then
    exit 0
fi

cargo build --release --manifest-path "$swap/.tools/zolana/Cargo.toml" -p spr-cli -p xark-cli
echo "Zolana CLI: $swap/.tools/zolana/target/release/zolana"
echo "Xark CLI: $swap/.tools/zolana/target/release/xark"

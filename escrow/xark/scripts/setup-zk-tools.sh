#!/usr/bin/env bash
# Prepare the exact SDK source used by this example and build its tools.
set -euo pipefail

escrow=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana_revision=98948daa1d4043515387041ab42e15961799058d
xark_revision=13c3f0c28eb202bc34f075ee5414a01c3cac51b8

checkout() {
    local repository=$1 revision=$2 directory=$3
    if [[ ! -e "$directory/.git" ]]; then
        mkdir -p "$directory"
        git -C "$directory" init --quiet
        git -C "$directory" remote add origin "$repository"
    fi
    # Fetch on a fresh checkout and when a pin bump left an older revision.
    if [[ $(git -C "$directory" rev-parse --verify --quiet HEAD) != "$revision" ]]; then
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
        if grep -F "$repository" "$file" | grep -F "git =" | grep -vqE "rev *= *\"$revision\""; then
            echo "$file references $repository without pinning rev = \"$revision\"" >&2
            exit 1
        fi
    done
}

checkout https://github.com/helius-labs/zolana.git "$zolana_revision" "$escrow/.tools/zolana"
check_pins https://github.com/helius-labs/zolana "$zolana_revision" "$escrow/Cargo.toml"
# The SDK, the generated circuit crates and this workspace share the crates.io
# Xark 0.3.0 release. Only Zolana's xark-cli comes from the ananas-block/xark
# fork, so this pin only guards the CLI build.
check_pins https://github.com/ananas-block/xark "$xark_revision" \
    "$escrow/Cargo.toml" "$escrow/.tools/zolana/Cargo.toml"

if [[ ${1:-} == --prepare-only ]]; then
    exit 0
fi

cargo build --release --manifest-path "$escrow/.tools/zolana/Cargo.toml" -p spr-cli -p xark-cli
echo "Zolana CLI: $escrow/.tools/zolana/target/release/zolana"
echo "Xark CLI: $escrow/.tools/zolana/target/release/xark"

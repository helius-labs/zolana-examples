#!/usr/bin/env bash
# Prepare the exact SDK source used by this example and build its tools.
set -euo pipefail

swap=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
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

checkout https://github.com/helius-labs/zolana.git "$zolana_revision" "$swap/.tools/zolana"
check_pins https://github.com/helius-labs/zolana "$zolana_revision" "$swap/Cargo.toml"
# The SDK, the generated circuit crates and this workspace share the crates.io
# Xark release. Only Zolana's xark-cli comes from the ananas-block/xark fork, so
# that pin only guards the CLI build.
check_pins https://github.com/ananas-block/xark "$xark_revision" \
    "$swap/Cargo.toml" "$swap/.tools/zolana/Cargo.toml"
if ! grep -F "ananas-block/xark" "$swap/.tools/zolana/Cargo.toml" | grep -qF "rev = \"$xark_revision\""; then
    echo "Zolana no longer pins xark-cli to $xark_revision" >&2
    exit 1
fi
xark_version() {
    grep -E '^xark = ' "$1" | grep -oE '"[0-9][^"]*"' | head -n 1
}
if [[ $(xark_version "$swap/Cargo.toml") != $(xark_version "$swap/.tools/zolana/Cargo.toml") ]]; then
    echo "$swap/Cargo.toml and Zolana use different xark versions" >&2
    exit 1
fi

if [[ ${1:-} == --prepare-only ]]; then
    exit 0
fi

cargo build --release --manifest-path "$swap/.tools/zolana/Cargo.toml" -p spr-cli -p xark-cli
echo "Zolana CLI: $swap/.tools/zolana/target/release/zolana"
echo "Xark CLI: $swap/.tools/zolana/target/release/xark"

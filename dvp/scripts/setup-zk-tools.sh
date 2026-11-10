#!/usr/bin/env bash
# Prepare the exact SDK source used by this example and build its tools.
set -euo pipefail

dvp=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana_revision=98948daa1d4043515387041ab42e15961799058d
xark_version=0.3.0
xark_cli_revision=13c3f0c28eb202bc34f075ee5414a01c3cac51b8

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
# The SDK and this workspace must share the released Xark crates; only the
# Zolana CLI builds from the Xark fork.
check_pins https://github.com/ananas-block/xark "$xark_cli_revision" "$dvp/.tools/zolana/Cargo.toml"
if grep -qF https://github.com/ananas-block/xark "$dvp/Cargo.toml"; then
    echo "$dvp/Cargo.toml must use the crates.io Xark release, not the fork" >&2
    exit 1
fi
for crate in xark xark-groth16; do
    for file in "$dvp/Cargo.toml" "$dvp/.tools/zolana/Cargo.toml"; do
        if ! grep -qx "$crate = \"$xark_version\"" "$file"; then
            echo "$file does not pin $crate = \"$xark_version\"" >&2
            exit 1
        fi
    done
done

if [[ ${1:-} == --prepare-only ]]; then
    exit 0
fi

cargo build --release --manifest-path "$dvp/.tools/zolana/Cargo.toml" -p spr-cli -p xark-cli
echo "Zolana CLI: $dvp/.tools/zolana/target/release/zolana"
echo "Xark CLI: $dvp/.tools/zolana/target/release/xark"

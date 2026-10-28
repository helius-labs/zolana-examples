#!/usr/bin/env bash
# Prepare the exact SDK and compiler sources used by this example.
set -euo pipefail

dvp=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
zolana_revision=226ddcb7ea4bdd1e328bf523a573f7c8d63d40eb
xark_revision=38c0de6fec56cff52f5a4e0cf2d86721ea29e583

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

checkout https://github.com/helius-labs/zolana.git "$zolana_revision" "$dvp/.tools/zolana"
checkout https://github.com/ananas-block/xark.git "$xark_revision" "$dvp/.tools/xark"

# This SDK revision records a developer's Xark paths. Resolve them against
# the pinned sibling checkout without changing the SDK implementation.
python3 - "$dvp/.tools/zolana/Cargo.toml" <<'PY'
import pathlib
import re
import sys

manifest = pathlib.Path(sys.argv[1])
source = manifest.read_text()
crates = {"xark": "lang", "xark-groth16": "groth16", "xark-wasm": "wasm"}
source, count = re.subn(
    r'(?m)^(xark(?:-groth16|-wasm)?) = \{ path = "[^"]+" \}$',
    lambda match: f'{match[1]} = {{ path = "../xark/crates/{crates[match[1]]}" }}',
    source,
)
if count != len(crates):
    raise SystemExit("The pinned Zolana manifest has unexpected Xark dependencies")
manifest.write_text(source)
PY

if [[ ${1:-} == --prepare-only ]]; then
    exit 0
fi

# Both security fixes are committed in the pinned Xark revision. Require them
# before building; keep SDK validation files matched to the unmodified source.
for name in input-region-indices proof-randomness; do
    git -C "$dvp/.tools/xark" apply --reverse --check "$dvp/.tools/zolana/tools/xark/$name.patch"
done

"$dvp/.tools/zolana/tools/xark/build.sh" "$dvp/.tools/xark"
cargo build --release --locked --manifest-path "$dvp/.tools/zolana/Cargo.toml" -p zolana-cli
echo "Zolana CLI: $dvp/.tools/zolana/target/release/zolana"
echo "Xark source: $dvp/.tools/xark (export XARK_SOURCE to this path)"

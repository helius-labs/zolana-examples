#!/usr/bin/env bash
# Runs a go command in the DvP circuits module, e.g.
#   dvp/scripts/circuits-go.sh vet ./...
#   dvp/scripts/circuits-go.sh test ./dvpterms/...
#   dvp/scripts/circuits-go.sh mod tidy
#
# The committed go.mod requires the gnark FFI bridge without replacing it (the
# prover build script supplies the replace at build time), so the command runs
# against a copy of go.mod/go.sum with the replace added, as zolana's
# `just example-circuits-go` does. Changes the command makes to the copy are
# written back without the replace.
#
# ZOLANA_DIR is the zolana v0.4.0-alpha source tree the go.mod replaces point
# at; it defaults to ../../../../zolana relative to the circuits module.
set -euo pipefail

circuits="$(cd "$(dirname "${BASH_SOURCE[0]}")/../prover/circuits" && pwd)"
zolana="${ZOLANA_DIR:-$circuits/../../../../zolana}"
bridge="$(cd "$zolana/sdk-libs/gnark-ffi-prover/build-helper/go" && pwd)"

export GOWORK=off
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp "$circuits/go.mod" "$tmp/circuits.mod"
cp "$circuits/go.sum" "$tmp/circuits.sum"

cd "$circuits"
go mod edit -replace="zolana/gnarkffiprover=$bridge" "$tmp/circuits.mod"
GOFLAGS="-modfile=$tmp/circuits.mod" go "$@"
go mod edit -dropreplace=zolana/gnarkffiprover "$tmp/circuits.mod"
cmp -s "$tmp/circuits.mod" go.mod || cp "$tmp/circuits.mod" go.mod
cmp -s "$tmp/circuits.sum" go.sum || cp "$tmp/circuits.sum" go.sum

#!/usr/bin/env bash
set -euo pipefail

staking_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
zolana_revision=92858c801026d78dde7b3203444050ca7a8adb34
zolana_checkout="$staking_root/.deps/zolana"

mkdir -p "$staking_root/.deps"
if [[ ! -d "$zolana_checkout/.git" ]]; then
  git init -q "$zolana_checkout"
  git -C "$zolana_checkout" remote add origin https://github.com/helius-labs/zolana
fi
if [[ "$(git -C "$zolana_checkout" rev-parse HEAD 2>/dev/null || true)" != "$zolana_revision" ]]; then
  git -C "$zolana_checkout" fetch --depth=1 origin "$zolana_revision"
  git -C "$zolana_checkout" checkout --detach "$zolana_revision"
fi

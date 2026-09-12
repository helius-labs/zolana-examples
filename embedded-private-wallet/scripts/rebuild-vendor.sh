#!/usr/bin/env bash
set -euo pipefail
example_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_dir="$(mktemp -d)"
# The reference wallet-kit workspace expects a sibling zolana-tvc checkout.
git clone https://github.com/helius-labs/zolana-tvc.git "$source_dir/zolana-tvc"
git -C "$source_dir/zolana-tvc" checkout --detach 35dafa8ad8afcbdb6099517f8b4f9db899a112a2
git clone https://github.com/helius-labs/wallet-kit.git "$source_dir/wallet-kit"
git -C "$source_dir/wallet-kit" checkout --detach c00f7a5e5ee7a2e2013c8301c762f870a99f8ee7
(cd "$source_dir/zolana-tvc" && pnpm install --frozen-lockfile --ignore-scripts && pnpm --filter @zolana/tvc-wallet build)
(cd "$source_dir/wallet-kit" && pnpm install --frozen-lockfile --ignore-scripts && pnpm --filter helius-wallet-kit build)
(cd "$source_dir/zolana-tvc/packages/tvc-wallet" && pnpm pack --pack-destination "$example_dir/vendor")
(cd "$source_dir/wallet-kit/packages/wallet-kit" && pnpm pack --pack-destination "$example_dir/vendor")
(cd "$example_dir" && shasum -a 256 vendor/*.tgz > vendor/SHA256SUMS)

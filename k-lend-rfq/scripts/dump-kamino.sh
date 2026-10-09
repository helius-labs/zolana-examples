#!/usr/bin/env bash
# Dump the Kamino kVault program and the Kamino lending program it requires
# from mainnet into k-lend-rfq/target/deploy, where the localnet tests load them.
# Skips a binary that is already there. A dump is zero-padded to the program
# account size, so it is cut at the end of the ELF section header table. Set
# KAMINO_DUMP_RPC_URL to dump through another RPC.
set -euo pipefail

url="${KAMINO_DUMP_RPC_URL:-https://api.mainnet-beta.solana.com}"
out_dir="$(cd "$(dirname "$0")/.." && pwd)/target/deploy"
mkdir -p "$out_dir"
for entry in KvauGMspG5k6rtzrqqn7WNn3oZdyKqLKwK2XWQ8FLjd:kamino_vault KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD:kamino_lending; do
    program="${entry%%:*}"
    out="$out_dir/${entry##*:}.so"
    [[ -f "$out" ]] && continue
    dump="$(mktemp)"
    solana program dump "$program" "$dump" --url "$url"
    shoff=$(od -An -t u8 -j 40 -N 8 "$dump" | tr -d ' ')
    shentsize=$(od -An -t u2 -j 58 -N 2 "$dump" | tr -d ' ')
    shnum=$(od -An -t u2 -j 60 -N 2 "$dump" | tr -d ' ')
    head -c $((shoff + shentsize * shnum)) "$dump" > "$out"
    rm -f "$dump"
done

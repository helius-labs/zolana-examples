#!/usr/bin/env bash
# Dump the SPL stake pool program and the token and stake programs it invokes
# from mainnet into staking-program/target/deploy, where the Mollusk tests load
# them. Skips a binary that is already there. A dump is zero-padded to the
# program account size, so it is cut at the end of the ELF section header
# table. Set STAKE_POOL_DUMP_RPC_URL to dump through another RPC.
set -euo pipefail

url="${STAKE_POOL_DUMP_RPC_URL:-https://api.mainnet-beta.solana.com}"
out_dir="$(cd "$(dirname "$0")/.." && pwd)/target/deploy"
mkdir -p "$out_dir"
for entry in \
    SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy:spl_stake_pool \
    TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA:spl_token \
    Stake11111111111111111111111111111111111111:solana_stake_program; do
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

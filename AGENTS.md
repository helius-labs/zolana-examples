# zolana-examples

## Solana code style

- Never assemble an instruction by hand with hardcoded byte offsets, `Buffer.alloc` + `writeUInt*`, or magic discriminator numbers. That is a sign of generated junk code.
- Build instructions the way the reference code does:
  - TypeScript: Solana Kit codecs and account roles (`getStructEncoder`, `getU8Encoder`, `AccountRole`, `Instruction`). In this repo: `typescript-client/src/lib.ts` and `typescript-client/examples/`. Upstream: https://github.com/helius-labs/zolana/blob/main/sdk-libs/ts/src/wallet/registry.ts
  - Rust: the client patterns in `rust-client/src/` and `rust-client/examples/`. Programs: Anchor `#[derive(Accounts)]`, `Context<T>`, program CPI helpers as in `escrow-program/program/` and `swap-program/program/`. No manual account-slice indexing or hand-built `Instruction { data: vec![...] }`.
  - Legacy web3.js only when the surrounding code already uses it, and then with the same helpers that code uses.
- Before writing or changing instruction-building, account, or CPI code, open the matching file in this repo or one of these repos and copy its pattern. Say which file you used.
  - Zolana main: https://github.com/helius-labs/zolana
  - Zolana examples: https://github.com/helius-labs/zolana-examples
  - Light Protocol program examples: https://github.com/Lightprotocol/examples-zk-compression/tree/main/program-examples
  - Light Protocol monorepo: https://github.com/Lightprotocol/light-protocol
  - Blueshift Anchor/Pinocchio patterns: https://github.com/blueshift-gg and https://learn.blueshift.gg
  - Solana Kit: https://github.com/anza-xyz/kit
  - Anchor: https://github.com/solana-foundation/anchor
- If a pattern is not in any of these, stop and ask before inventing one.

# Sync-SDK edits

Amend existing files. Do not redesign examples.
Read sha and html_url from the payload section at the end of this prompt. Do not invent a rev.

## Allowlist (these paths only)

- typescript-client/package.json
- typescript-client/pnpm-lock.yaml
- rust-client/Cargo.toml
- rust-client/Cargo.lock
- typescript-client/examples/deposit_transfer_withdraw.ts
- rust-client/examples/deposit_transfer_withdraw.rs

The pin files are already updated to the payload sha. Do not change them unless a lockfile refresh is required for compile.

## What stays

- package name `@heliuslabs/zolana`
- rust-client Cargo.toml comment that says pins are git revs
- 4 rust git deps, same crates and features; only `rev` may change
- swap-program/** and escrow-program/**
- READMEs, comments, formatting, CI YAML

## Sources only if needed

1. Run `pnpm check` in typescript-client and `cargo check --all-targets --locked` in rust-client
2. If both pass: stop. Do not open the .ts / .rs files
3. If fail: change the smallest expression the compiler names in the failing example file
4. A comment changes only when that line was deleted

## Forbidden

- new files, extra deps, format/README, improve/clarify
- swap-program, escrow-program, .github

## Done

`git diff --name-only` is a subset of the allowlist. Checks green.

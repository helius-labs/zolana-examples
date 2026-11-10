# Sync-SDK edits

Amend existing files. Do not redesign examples.
Read sha and html_url from the payload section at the end of this prompt. Do not invent a rev.

## Allowlist (these paths only)

- typescript-client/examples/deposit_transfer_withdraw.ts
- rust-client/examples/deposit_transfer_withdraw.rs

The workflow has already pinned the payload sha everywhere: the TS package,
every `helius-labs/zolana` Git rev in every Cargo.toml (gnark, Xark and
rust-client) with their lockfiles, `zolana_revision` in
`test-utils/zolana-release.sh`, and the gnark-sdk and prover/server
pseudo-versions in both gnark `go.mod` files, and refreshed every lockfile.
Do not change any pin, manifest or lockfile; the workflow fails on any edit
outside the allowlist, including new files.

## What stays

- package name `@heliuslabs/spr`
- typescript-client/package.json, typescript-client/pnpm-lock.yaml
- rust-client/Cargo.toml (its comment points at `zolana_revision`, so it
  names no commit) and rust-client/Cargo.lock
- swap/** and escrow/**, including their Cargo and Go pins
- test-utils/**
- READMEs, comments, formatting, CI YAML

## Sources only if needed

1. Run `pnpm check` in typescript-client and `cargo check --all-targets --locked` in rust-client
2. If both pass: stop. Do not open the .ts / .rs files
3. If fail: change the smallest expression the compiler names in the failing example file
4. A comment changes only when that line was deleted

## Forbidden

- new files, extra deps, format/README, improve/clarify
- swap, escrow, test-utils, .github

## Done

`git diff --name-only` of your edits is a subset of the allowlist. Checks green.

# Rust Client SDK Examples

End-to-end client examples for the shielded pool, driven through the
`zolana-client` SDK against a local validator, Photon indexer, and prover. Each
example is a self-contained binary covering one operation. The shared harness in
`src/lib.rs` handles only the localnet and protocol-admin setup a real app never
writes, plus the deposit seeding the transfer, withdraw, and sync examples need;
each example holds the SDK call it demonstrates so you see the code production
holds.

## Examples

- **`deposit`** (proofless): deposit a public SPL balance into the pool with
  `create_deposit`, sent at the instruction level.
- **`transfer`** (private send): move an SPL value privately between two private
  balances, spending one note and one SOL fee note. Proven.
- **`withdraw`**: withdraw an SPL value back to a public account's ATA, building
  the SPL withdrawal target by hand. Proven.
- **`sync_balance`**: query the indexer for a wallet's encrypted UTXOs by view
  tag, the raw layer under `sync_wallet`.

Transfer and withdraw build the transaction by hand: select inputs, add the
output, sign, then let the one-call `Submit` action fetch the merkle and
non-inclusion proofs, assemble the witness, prove, and send the `Transact`
instruction.

The wallet owns its `AssetRegistry`, so the SDK reads asset ids off the wallet
(`sync_wallet`, `get_private_token_balances`, and the transaction builder take no
separate registry argument). Register SPL assets before creating the parties that
spend them.

## Prerequisites

Build the on-chain programs, prover, CLI, and indexer once:

```bash
just build-programs build-prover-server build-cli
just ensure-photon
just ensure-smart-account
```

`ensure-photon` builds the Photon indexer from a sibling `../photon` checkout
(`just build-photon` -> `target/bin/photon`); point `ZOLANA_PHOTON_BIN` at a
prebuilt binary to skip the build.

Transfers and withdrawals generate a proof; the prover downloads its proving
keys from a GitHub release on first use, which needs `gh` authenticated for the
hosting org (`gh auth status`). Deposits and sync are proofless and need
neither, so they run with no `gh` and no keys.

## Run

Each example boots its own validator, Photon, and prover, so run one at a time:

```bash
just run-rust-client-example deposit
```

or directly:

```bash
cargo run -p rust-client-example --example transfer
```

Start with `deposit` or `sync_balance`: they are proofless, so they validate the
setup without the prover or `gh`. Each example prints a single `ok ...` line with
the transaction signature and the resulting balance.

The validator binds the RPC port (8899) and Photon port (8784). The
`just run-rust-client-example` recipe frees stale validators on those ports
before each run; a bare `cargo run` does not, so kill leftover processes first if
a port is busy.

## Documentation

- Protocol spec: [`docs/spec.md`](../../zolana-devx/docs/spec.md)
- Client SDK: [`sdk-libs/client`](../../zolana-devx/sdk-libs/client)

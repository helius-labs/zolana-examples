# Timelock Escrow Program

A timelock escrow on the Solana Privacy Program (SPP). The creator locks funds
as a shielded UTXO with a chosen unlock timestamp and reclaims them once that
timestamp has passed. The same creator that locks the funds is the only party
that can withdraw them. Amounts stay private.

The timelock escrow program is an SPP ZK program: it verifies a small proof of
its own escrow rules and delegates the confidential transfer to SPP. It stores
no state and owns no accounts.

See [`timelock_escrow.md`](timelock_escrow.md) for the full design: the
privacy model, escrow terms, instructions, and circuits.

## Layout

- [`program/`](program/) — the Pinocchio program. Verifies a Groth16 proof
  against the public-input hash, then CPIs SPP `transact`.
- [`prover/`](prover/) — in-process proving engine. Go gnark circuits, ffi
  bindings, and the key-generation binary.
- [`sdk/`](sdk/) — client library. State, instruction and proof-input
  builders, UTXO hashing, and the prover client.
- [`test/`](test/) — localnet end-to-end tests and CU benchmarks
  ([`BENCHMARK.md`](BENCHMARK.md)).

## Build

The prover compiles Go gnark circuits, so building needs a Go toolchain
alongside Rust.

```bash
cargo build
```

The circuit tests need the pinned proving and verifying keys, whose hashes are
in [`timelock-escrow-keys.CHECKSUM`](timelock-escrow-keys.CHECKSUM). Generate
them with the `timelock-escrow-prover-setup` binary.

Build the program for deployment:

```bash
cd program && cargo build-sbf --tools-version v1.54 -- --features bpf-entrypoint
```

## Test

### Requirements

- Go toolchain (the prover compiles gnark circuits)
- solana cli version 4.x (for `cargo build-sbf`)

### Running Tests

Run unit and circuit tests with `cargo test` in `program/` and `sdk/`.

The end-to-end tests in [`test/`](test/) run on localnet: the harness spawns
a solana test validator, photon indexer, and prover server through the
`zolana` cli. It expects built zolana artifacts in the directory above this
repository — the shielded-pool, user-registry, and Squads programs in
`target/deploy/` and the SPP proving keys in `prover/server/proving-keys/`.
`ZOLANA_CLI_BIN`, `ZOLANA_PHOTON_BIN`, `ZOLANA_PROVER_BIN`, and
`TIMELOCK_ESCROW_PROGRAM_SO` override the binary paths.

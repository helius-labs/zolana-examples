# Swap Program

A confidential swap between a maker and a taker on the Solana Privacy Program
(SPP). The maker commits an order that locks the funds it is selling as a
shielded UTXO; the taker takes it before expiry, or the maker reclaims it
after. Amounts and the price stay private. That a swap was made and later
taken or cancelled is public.

The swap program is an SPP ZK program: it verifies a small proof of its own
swap rules and delegates the confidential transfer to SPP. It stores no state
and owns no accounts.

See [`swap_program.md`](swap_program.md) for the full design: the privacy
model, order terms, instructions, and circuits.

## Layout

- [`program/`](program/) — the Pinocchio program. Verifies a Groth16 proof
  against the public-input hash, then CPIs SPP `transact`.
- [`prover/`](prover/) — in-process proving engine. Go gnark circuits, ffi
  bindings, and the key-generation binary.
- [`sdk/`](sdk/) — client library. State, instruction and proof-input
  builders, UTXO hashing, discovery, encryption codecs, and the prover client.
- [`test/`](test/) — localnet end-to-end tests and CU benchmarks
  ([`BENCHMARK.md`](BENCHMARK.md)).

## Build

The prover compiles Go gnark circuits, so building needs a Go toolchain
alongside Rust.

```bash
cargo build
```

The circuit tests need the pinned proving and verifying keys, whose hashes are
in [`swap-keys.CHECKSUM`](swap-keys.CHECKSUM). Generate them with the
`swap-prover-setup` binary.

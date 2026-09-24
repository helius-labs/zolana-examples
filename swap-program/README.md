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
- [`prover/`](prover/) — in-process proving engine on the shared
  `zolana-gnark-ffi-prover`. Go gnark circuits, their registration, and the
  key-generation binary.
- [`sdk/`](sdk/) — client library. State, instruction and proof-input
  builders, UTXO hashing, discovery, encryption codecs, and the prover client.
- [`test/`](test/) — localnet end-to-end tests and CU benchmarks
  ([`BENCHMARK.md`](BENCHMARK.md)).

## Build

Each crate is its own package with no shared workspace, so build them one at a
time. The Zolana crates come from the `v0.3.0-alpha` tag of
[helius-labs/zolana](https://github.com/helius-labs/zolana). Run the commands
below from `swap-program/`.

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint
cargo build --manifest-path sdk/Cargo.toml
```

The prover compiles Go gnark circuits, so building `prover`, `sdk` or `test`
needs Go 1.27.1 or newer alongside Rust. The circuits import the `zolana/prover`
and `zolana/gnarksdk` Go modules, which
[`prover/circuits/go.mod`](prover/circuits/go.mod) takes from a Zolana checkout
named `zolana` next to the `zolana-examples` checkout:

```bash
git clone --branch v0.3.0-alpha https://github.com/helius-labs/zolana ../../zolana
```

The Go bridge to Rust ships inside the `zolana-gnark-ffi-prover-build` crate
and needs no checkout.

The circuit tests need the pinned proving and verifying keys, whose hashes are
in [`swap-keys.CHECKSUM`](swap-keys.CHECKSUM). They are insecure test keys from
a fixed public seed. That is the recommended setup for integration tests, and it
is secure there because a test validator holds nothing a forged proof could
take. They are UNSAFE for production. Generate them into `build/gnark/` with the
`swap-prover-setup` binary:

```bash
for c in make take cancel take_verifiable_encryption; do
  cargo run --manifest-path prover/Cargo.toml --bin swap-prover-setup -- \
    "$c" "build/gnark/$c" --insecure-test-keys
done
```

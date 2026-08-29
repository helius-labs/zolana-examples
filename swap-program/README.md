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
alongside Rust. The circuits import shared gadgets from the
[zolana](https://github.com/helius-labs/zolana) monorepo through a local
`replace` that resolves to `prover/server` four directories above
`prover/circuits`. Check out the monorepo at the rev pinned in the Cargo
manifests and place this repository inside its root. A `prover` symlink next
to this repository also satisfies the build and the circuit tests, but the
localnet tests resolve every artifact from the monorepo root and need the
nested checkout.

```bash
cargo build
```

The circuit tests and the localnet tests need the pinned proving and verifying
keys, whose hashes are in [`swap-keys.CHECKSUM`](swap-keys.CHECKSUM). Download
them from the `swap-keys-v4` release:

```bash
for c in make take cancel take_verifiable_encryption; do
  for k in pk vk; do
    gh release download swap-keys-v4 --repo helius-labs/zolana \
      --pattern "${c}_${k}.bin" --output "build/gnark/$c/$k.bin"
  done
done
```

Groth16 setup is randomized, so keys generated with `swap-prover-setup` do not
match the committed verifying keys. Use that binary only to rotate the keys
together with the committed verifying keys and the checksum manifest.

The localnet tests in [`test/`](test/) start the validator, Photon, and the
prover through the monorepo `zolana` CLI. They locate the binaries through
`ZOLANA_CLI_BIN`, `ZOLANA_PHOTON_BIN`, and `ZOLANA_PROVER_BIN`, and the program
binary through `SWAP_PROGRAM_SO`.

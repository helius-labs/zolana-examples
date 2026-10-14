# Private DvP

A private delivery-versus-payment trade on the Solana Privacy Program (SPP).
The seller and buyer agree on terms, and each funds its leg as a shielded
deposit owned by the DvP PDA. A settlement authority then settles both legs in
one SPP transaction, or neither moves. Until then the settlement authority can
cancel the DvP and either party can reject it, which refunds both deposits; a
party can also reclaim its own deposit at any time. The terms stay confidential
to the seller, buyer and settlement authority. That a DvP was created and later
settled or aborted is public.

The DvP program is an SPP ZK program: each instruction verifies a proof of its
own rules and CPIs SPP `transact` for the confidential transfer. It stores no
state and owns no accounts.

See [`spec.md`](spec.md) for the full design: the privacy model, terms,
instructions, and circuits.

## Deviations from the spec

`spec.md` is left as written. The implementation departs from it where SPP at
`v0.4.0-alpha` requires it, or fills in details the spec leaves open. The code
cites these by number.

- **D1.** Create and reclaim prove at SPP shape 1x2 with a compact padding
  output; SPP has no 1x1 shape.
- **D2.** A change or refund output with no value is a zero-amount UTXO owned
  by its refund destination, not an empty UTXO.
- **D3.** Abort inputs put the deposits first and padding last, as SPP
  requires of dummies. A private bit records which leg is in input slot 1. The
  outputs stay fixed by leg.
- **D4.** `dvp_id` is a nested Poseidon hash of the terms, since Poseidon
  takes at most 12 inputs (see `sdk/src/state/swap_dvp.rs`).
- **D5.** Timestamps must be non-negative: the circuits range-check them to 63
  bits, and the program rejects a negative `current_timestamp`.
- **D6.** The create address slot is pinned to tree id 0, so each
  `(creator, dvp_id)` can be created once.
- **D7.** Terms message `j` uses ciphertext slot `1 + j`, and its data is the
  bare ciphertext. The AES-CTR message is not authenticated; a recipient
  trusts it only after the terms hash to the committed DvP UTXO.
- **D8.** `dvp_authority` is forwarded as the last SPP account (the
  owner-signer run), as in the escrow and swap examples.
- **D9.** The DvP UTXO output carries no ciphertext; the parties recompute it
  from the terms.
- **D10.** Settlement and refund destinations must have Ed25519 or PDA signing
  keys; the sdk refuses P256 ones.

The program also requires the canonical `dvp_authority` bump. The spec only
says the program checks the bump; another bump would give another PDA, so the
same `(creator, dvp_id)` could be created twice.

## Layout

- [`program/`](program/) — the Pinocchio program with the `create_dvp`,
  `reclaim_dvp`, `settle_dvp`, `cancel_dvp` and `reject_dvp` instructions.
  Each verifies a Groth16 proof against the public-input hash, then CPIs SPP
  `transact`.
- [`prover/`](prover/) — in-process proving engine on the shared
  `zolana-gnark-ffi-prover`. Go gnark circuits, their registration, and the
  `dvp-prover-setup` key-generation binary.
- [`sdk/`](sdk/) — client library. Terms, instruction and proof-input
  builders, UTXO hashing, discovery of terms and unspent deposits, and
  encryption codecs.
- [`test/`](test/) — localnet end-to-end tests and CU benchmarks
  ([`BENCHMARK.md`](BENCHMARK.md)).

## Prerequisites

- Rust stable.
- The Agave tools with `cargo-build-sbf` and platform tools v1.53 or v1.52,
  not v1.54 (see [Build](#build)).
- Go 1.27.1 or newer. The prover compiles Go gnark circuits, so building
  `prover`, `sdk` or `test` needs it.
- The zolana `v0.4.0-alpha` source. The Rust crates come from that tag by git.
  The circuits import the `zolana/prover` and `zolana/gnarksdk` Go modules,
  which [`prover/circuits/go.mod`](prover/circuits/go.mod) takes from a
  directory named `zolana` next to the `zolana-examples` checkout. Unpack the
  release source archive there, from `dvp/`:

  ```bash
  mkdir ../../zolana
  curl -sSfL https://github.com/helius-labs/zolana/archive/refs/tags/v0.4.0-alpha.tar.gz \
    | tar -xz -C ../../zolana --strip-components=1
  ```

## Build

The crates share one workspace. Run the commands below from `dvp/`.

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.53 -- --features bpf-entrypoint
cargo build --workspace
```

Build the program with `--features bpf-entrypoint`. Without it `cargo
build-sbf` writes a `.so` with no entrypoint to `target/deploy`, and the
end-to-end tests then fail with `AccountNotFound` for the program id.
Do not use platform tools v1.54: its build fails every instruction on chain
with `InvalidInstructionData`, because serializing the SPP transact data
reports a `PreallocationSizeLimit` error.

## Test keys

The circuit and end-to-end tests need the proving and verifying keys of the
four circuits, whose hashes are in [`dvp-keys.CHECKSUM`](dvp-keys.CHECKSUM).
They are insecure test keys from a fixed public seed. That is the recommended
setup for integration tests, and it is secure there because a test validator
holds nothing a forged proof could take. They are UNSAFE for production.
Generate them into `build/gnark/`:

```bash
for c in create_dvp reclaim_dvp settle_dvp abort_dvp; do
  cargo run -p dvp-prover --bin dvp-prover-setup -- \
    "$c" "build/gnark/$c" --insecure-test-keys
done
```

`abort_dvp` is the circuit of both `cancel_dvp` and `reject_dvp`. Each run
writes `pk.bin` and `vk.bin`; the SHA-256 of `build/gnark/<c>/pk.bin` must match
the `<c>_pk.bin` line of the checksum file, and likewise for `vk.bin`.

The program's verifying keys are committed in
[`program/src/verifying_keys/`](program/src/verifying_keys/). After a circuit
change, add `--rust-vk program/src/verifying_keys/$c.rs` to the setup command to
regenerate them, update `dvp-keys.CHECKSUM`, and rewrite the constraint counts
the benchmark reports with
`scripts/circuits-go.sh test . -run TestConstraintCounts -update`.

## Tests

The program tests run under mollusk against the SBF build:

```bash
cargo test-sbf --tools-version v1.53 -p dvp-program --features bpf-entrypoint
```

The SDK tests prove and check each circuit with the test keys:

```bash
cargo test -p dvp-prover -p dvp-sdk
```

The Go circuit tests run through [`scripts/circuits-go.sh`](scripts/circuits-go.sh),
which adds the module replace for the gnark FFI bridge to a temporary copy of
`go.mod`. The zolana source must be at `../../zolana`, where the `go.mod`
replaces point.

```bash
scripts/circuits-go.sh vet ./...
scripts/circuits-go.sh test ./...
```

The end-to-end tests in `test/` boot a localnet with surfpool, Photon and the
prover, deploy SPP and `target/deploy/dvp_program.so`, and run each flow. They
read the binaries from these variables:

| Variable | Binary |
|----------|--------|
| `ZOLANA_CLI_BIN` | `zolana-<platform>-v0.4.0-alpha` |
| `PROVER_BIN` | `prover-<platform>-v0.4.0-alpha` |
| `ZOLANA_PHOTON_BIN` | `photon-<platform>-v0.4.0-alpha` |
| `SHIELDED_POOL_PROGRAM_PATH` | `shielded_pool_program-v0.4.0-alpha.so` |
| `SURFPOOL_BIN` | `surfpool` |

The first four are assets of the zolana
[`v0.4.0-alpha` release](https://github.com/helius-labs/zolana/releases/tag/v0.4.0-alpha)
(`gh release download v0.4.0-alpha -R helius-labs/zolana`), where
`<platform>` is `linux-x64` or `darwin-arm64`. `surfpool` is in the platform
archive of the Lightprotocol/surfpool
[`v1.6.0-light` release](https://github.com/Lightprotocol/surfpool/releases/tag/v1.6.0-light).
The SPP proving keys are read from `ZOLANA_PROVER_KEYS_DIR`, or
`~/.config/zolana/proving-keys` by default; the first run downloads the ones it
is missing. With the program built and the variables set:

```bash
cargo test -p dvp-test
```

## Benchmark

The CU benchmark replays the five instructions under mollusk with a profiled
program build in `target/bench`, kept out of `target/deploy` because a
validator does not register the profiler syscall. It reads the pool program
from `SHIELDED_POOL_PROGRAM_PATH` and gets the SPP proofs from a prover on
`ZOLANA_PROVER_URL`, or starts one with `ZOLANA_CLI_BIN`. It writes
[`BENCHMARK.md`](BENCHMARK.md).

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.53 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
cargo test -p dvp-test --test bench_cu -- --ignored --nocapture
```

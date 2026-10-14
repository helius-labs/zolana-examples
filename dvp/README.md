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

## Layout

- [`program/`](program/) — the Pinocchio program with the `create_dvp`,
  `reclaim_dvp`, `settle_dvp`, `cancel_dvp` and `reject_dvp` instructions.
  Each verifies a Groth16 proof against the public-input hash, then CPIs SPP
  `transact`.
- [`prover/`](prover/) — in-process proving engine on the shared
  `zolana-gnark-ffi-prover`. Go gnark circuits, their registration, and the
  `dvp-prover-setup` key-generation binary.
- [`sdk/`](sdk/) — client library. Terms, instruction and proof-input
  builders, UTXO hashing, deposit discovery, encryption codecs, and the prover
  client.
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
regenerate them, and update `dvp-keys.CHECKSUM`.

## Tests

The program tests run under mollusk against the SBF build:

```bash
cargo test-sbf -p dvp-program --features bpf-entrypoint
```

The SDK tests prove and check each circuit with the test keys:

```bash
cargo test -p dvp-prover -p dvp-sdk
```

The Go circuit tests run through [`scripts/circuits-go.sh`](scripts/circuits-go.sh),
which adds the module replace for the gnark FFI bridge to a temporary copy of
`go.mod`. Set `ZOLANA_DIR` if the zolana source is somewhere other than
`../../zolana`.

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
cargo build-sbf --manifest-path program/Cargo.toml --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
cargo test -p dvp-test --test bench_cu -- --ignored --nocapture
```

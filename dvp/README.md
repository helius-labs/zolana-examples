# Private DvP

A private delivery-versus-payment trade on the Solana Privacy Program (SPP).
The seller and buyer agree on terms and fund shielded deposits owned by the
DvP PDA. The settlement authority settles both deposits in one transaction.
It can also cancel the DvP, and either party can reject it; both operations
refund the included deposits. Each party can reclaim its own deposit at any
time. The terms stay confidential to the seller, buyer and settlement authority.

The circuits belong to the DvP program and use Zolana's Xark-backed
`ZkProgram` and `ClientInputs` derives, `TxContext` and `CheckedTransaction`.
Each circuit's `execute` method runs natively to build the transaction and
compiles through Xark for proving. The derives generate the Rust and
TypeScript transaction builders and on-chain verifiers.

See [`spec.md`](spec.md) for the design, and
[`settle_dvp_full_flow`](test/tests/settle_dvp.rs) for the Rust create, fund,
discover and settle flow. The [Node.js examples](typescript/examples/) show
settlement, reclaim, cancellation and rejection with each flow inlined.

## Layout

- [`program/src/circuits/`](program/src/circuits/) contains one file per circuit.
  `abort_dvp` serves both cancellation and rejection.
- [`program/src/instructions/`](program/src/instructions/) checks instruction
  accounts, verifies the program proof and invokes SPP `transact`.
- [`program/src/verifier/`](program/src/verifier/) contains generated verifiers.
- [`sdk/`](sdk/) contains Rust terms, instruction builders, deposit discovery
  and encryption codecs.
- [`typescript/`](typescript/) contains the Node.js client and examples.
- [`test/`](test/) contains localnet tests and the CU benchmark.

## Build

Use Rust 1.95.0, Node.js 24 or newer, Python 3, `wasm-pack`, and Agave with
SBF platform tools v1.54. `rust-toolchain.toml` pins the compiler used to
generate the checked bytecode. The workspace uses Zolana
[`226ddcb7e`](https://github.com/helius-labs/zolana/commit/226ddcb7ea4bdd1e328bf523a573f7c8d63d40eb)
and Xark
[`38c0de6`](https://github.com/ananas-block/xark/commit/38c0de6fec56cff52f5a4e0cf2d86721ea29e583).
The setup script checks out both revisions under `.tools/`, resolves the
SDK revision's developer-specific Xark paths against the sibling checkout,
and builds the matching CLI and compiler. Cargo and the TypeScript package
use these sources; run setup before any Cargo or npm command.

Run these commands from `dvp/`:

```bash
scripts/setup-zk-tools.sh
export ZOLANA_CLI_BIN="$PWD/.tools/zolana/target/release/zolana"
export XARK_SOURCE="$PWD/.tools/xark"
"$ZOLANA_CLI_BIN" zk compile --test --manifest-path program/Cargo.toml --wasm-target nodejs --verifier
scripts/check-keys.sh
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --locked --features bpf-entrypoint
cargo build --release --workspace --locked
npm --prefix .tools/zolana ci
npm --prefix .tools/zolana run build:ts
npm --prefix typescript ci
npm --prefix typescript run check
npm --prefix typescript test
```

Build with `bpf-entrypoint` and platform tools v1.54 to match the pinned SDK
and CI. `scripts/setup-zk-tools.sh --prepare-only` prepares sources without
building the tools.

## Circuit keys

`zolana zk compile --test` generates each circuit's `circuit.xbc`, `pk.bin`
and `vk.bin` under `target/zk/dvp-program/<circuit>/` and its on-chain verifier
in `program/src/verifier/`. The committed
[`dvp-keys.CHECKSUM`](dvp-keys.CHECKSUM) pins the circuit bytecode, proving and verifying keys
used by both clients. These are deterministic test keys from an insecure
test setup; use a trusted setup before deploying a production program.

Check that the circuits reproduce the committed verifiers and key checksums:

```bash
"$ZOLANA_CLI_BIN" zk compile --test --manifest-path program/Cargo.toml --check
scripts/check-keys.sh
```

After an intentional circuit change, regenerate the keys and verifiers with
`zolana zk compile --test`, then update the committed checksums with
`shasum -a 256` over the generated `*/circuit.xbc`, `*/pk.bin` and `*/vk.bin` files, relative
to `target/zk/dvp-program/`. `DVP_KEYS_DIR` can point the Rust client and
checksum script at another directory containing the same verified artifacts.

## Tests

```bash
cargo fmt -p dvp-program -p dvp-sdk -p dvp-test -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked -p dvp-program --features bpf-entrypoint
cargo test --release --locked -p dvp-sdk
```

The end-to-end tests start a localnet with surfpool, Photon and the SPP prover.
Download the protocol binaries from
[v0.5.0-alpha](https://github.com/helius-labs/zolana/releases/tag/v0.5.0-alpha),
the release pinned by this SDK revision's `cli/release-artifacts.lock`.
Set these paths for your platform:

| Variable | Asset |
|----------|-------|
| `ZOLANA_CLI_BIN` | `.tools/zolana/target/release/zolana` built above |
| `PROVER_BIN` | `prover-<platform>-v0.5.0-alpha` |
| `ZOLANA_PHOTON_BIN` | `photon-<platform>-v0.5.0-alpha` |
| `SHIELDED_POOL_PROGRAM_PATH` | `shielded_pool_program-v0.5.0-alpha.so` |
| `SURFPOOL_BIN` | `surfpool` from [v1.6.0-light](https://github.com/Lightprotocol/surfpool/releases/tag/v1.6.0-light) |

`<platform>` is `linux-x64` or `darwin-arm64`. The release's `SHA256SUMS`
records each asset's checksum. The prover downloads missing SPP keys to
`ZOLANA_PROVER_KEYS_DIR`, or `~/.config/zolana/proving-keys` by default.

```bash
mkdir -p ~/.config/zolana/proving-keys
cargo test --release --locked -p dvp-test
cargo test --release --locked -p dvp-test --test create_dvp node_client_lifecycles -- --ignored --nocapture
```

The second command runs all four Node.js examples against a funded localnet
fixture. Build the Node.js WASM module and install the TypeScript dependencies
before running it.

## Benchmark

Run native Rust proving and benchmarks with `--release`. Before the Xark
migration, debug builds were
17–28× slower in the measured DvP flows. Release proof times were 87 ms for
create, 158 ms for reclaim, 251 ms for settle, and 285 ms for cancel, including
checked key loading and prover construction. See [`BENCHMARK.md`](BENCHMARK.md)
for those historical results; regenerate the benchmark for the Xark backend.

Input counts are fixed: reclaim has one deposit; settle has the DvP UTXO
and two deposits; abort has those same three slots, with padding for absent
deposits. Create reserves one address and has no token UTXO inputs.
`TokenUtxos::new_init` allocates no input UTXOs. The circuit tests check these
witness counts so a larger padding default cannot add unused inputs.

The benchmark profiles the five instructions under Mollusk and writes
[`BENCHMARK.md`](BENCHMARK.md). It reads the SPP binary from
`SHIELDED_POOL_PROGRAM_PATH` and uses the prover at `ZOLANA_PROVER_URL`, or
starts one with `ZOLANA_CLI_BIN`. Keep its profiled program in `target/bench`,
because a validator does not register the profiler syscall.

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
cargo test --release --locked -p dvp-test --test bench_cu -- --ignored --nocapture
```

# Private DvP

A private delivery-versus-payment trade on the Solana Privacy Program (SPP).
The seller and buyer agree on terms and fund shielded deposits owned by the
DvP PDA. The settlement authority settles both deposits in one transaction.
It can also cancel the DvP, and either party can reject it; both operations
refund the included deposits. Each party can reclaim its own deposit at any
time. The terms stay confidential to the seller, buyer and settlement authority.

The circuits belong to the DvP program and use Zolana's `ZkProgram` macros,
`TxContext` and `CheckedTransaction`. The macros generate the Rust and
TypeScript transaction builders, provers and on-chain verifiers.

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

Use Rust stable, Node.js 24 or newer, `wasm-pack`, and Agave with SBF platform
tools v1.56. The workspace pins its Zolana dependencies to
[`ded44879`](https://github.com/helius-labs/zolana/commit/ded44879b3df35482d2871bc1d0f32f2d1be2879) (`v0.5.0-alpha`).
Download the compatible CLI and localnet binaries from the
[test release for PR #41](https://github.com/helius-labs/zolana/releases/tag/test-dvp-pr-41-20261017-48dd150).
The native artifacts use protocol sources at `48dd150`; the attached
TypeScript SDK test package uses
[`d4ec52316`](https://github.com/helius-labs/zolana/commit/d4ec5231689ecdf3421049e9855097bfaf966276).
The release records both source commits and all asset checksums. The
TypeScript dependency is pinned to that test package in `package-lock.json`.
Run these commands from `dvp/`, with the downloaded CLI on `PATH`:

```bash
zolana zk compile --manifest-path program/Cargo.toml --wasm-target nodejs --verifier
scripts/check-keys.sh
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.56 -- --locked --features bpf-entrypoint
cargo build --release --workspace --locked
npm --prefix typescript ci
npm --prefix typescript run check
npm --prefix typescript test
```

Build with `bpf-entrypoint` and platform tools v1.56 to match CI. Use a clean
target directory when changing platform tools, then run the SDK program-boundary
tests against the resulting binary.

The Rust instruction builders send the tagged 33-byte X25519 viewing public key.
The program combines it with the PDA owner hash to verify the same shielded
address committed by the circuit. Upgrade the Rust SDK and program together.

## Circuit keys

`zolana zk compile` generates the four circuits' proving keys in
`target/zk/dvp-program/` and their verifiers in `program/src/verifier/`.
The committed [`dvp-keys.CHECKSUM`](dvp-keys.CHECKSUM) pins the proving keys
used by both clients. The Zolana 0.5 Devnet upgrade uses fresh single-party keys and program ID
`7eGHQWQQMCuYBV3rGhMwasi2R1VmjLfYPA5jnUpjyGsb`. Use a trusted setup before
deploying a production program.

Check that the circuits reproduce the committed verifiers and key checksums:

```bash
zolana zk compile --manifest-path program/Cargo.toml --check
scripts/check-keys.sh
```

After an intentional circuit change, regenerate the keys and verifiers with
`zolana zk compile`, then update the committed checksums from the generated
`*.pk.sha256` files. `DVP_KEYS_DIR` can point the Rust client and checksum
script at another directory containing the same verified keys.

## Tests

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked -p dvp-program --features bpf-entrypoint
cargo test --release --locked -p dvp-sdk
```

The end-to-end tests start a localnet with surfpool, Photon and the SPP prover.
Set these paths to the test release assets for your platform:

| Variable | Asset |
|----------|-------|
| `ZOLANA_CLI_BIN` | `zolana-<platform>-test-dvp-pr-41-20261017-48dd150` |
| `PROVER_BIN` | `prover-<platform>-test-dvp-pr-41-20261017-48dd150` |
| `ZOLANA_PHOTON_BIN` | `photon-<platform>-test-dvp-pr-41-20261017-48dd150` |
| `SHIELDED_POOL_PROGRAM_PATH` | `shielded_pool_program-test-dvp-pr-41-20261017-48dd150.so` |
| `SURFPOOL_BIN` | `surfpool` from [v1.6.0-light](https://github.com/Lightprotocol/surfpool/releases/tag/v1.6.0-light) |

`<platform>` is `linux-x64` or `darwin-arm64`. The test release's
`SHA256SUMS` records each asset's checksum. `PROVENANCE.json` records the
source revisions, build tools and pinned external smart-account binary;
`proving-keys.lock` and the two platform artifact locks are attached as well. The prover downloads missing SPP keys to
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

Run native Rust proving and benchmarks with `--release`. Debug builds were
17–28× slower in the measured DvP flows. Release proof times were 87 ms for
create, 158 ms for reclaim, 251 ms for settle, and 285 ms for cancel, including
checked key loading and prover construction. See [`BENCHMARK.md`](BENCHMARK.md)
for the full results.

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
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.56 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
cargo test --release --locked -p dvp-test --test bench_cu -- --ignored --nocapture
```

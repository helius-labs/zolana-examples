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

## Implementations

- [`gnark/`](gnark/) — circuits written in Go with gnark, proved in process
  through `spr-gnark-ffi-prover`.
- [`xark/`](xark/) — circuits written in Rust with Zolana's Xark-backed
  program SDK, proved in process from checksum-pinned Xark artifacts.

Both implement the same instructions, tags, errors, order terms, optional
designated taker and test scenarios. They differ in program ID, in how each
circuit's public values fold into its one public input (gnark hashes
`Poseidon(private_tx_hash, values...)`, make exposing `private_tx_hash`
itself; Xark hashes `Poseidon(values..., transaction_hash)`), in how the
order UTXO carries the terms, and in who owns make's source: Xark stores the
terms as a program SDK state (`DataUtxo<OrderTerms>`), so its data hash is the
SDK state hash and its encrypted payload the Borsh-encoded terms, and its make
spends a maker-owned source that the maker signs for, with the maker signer as
a public field (see [xark](#xark)). gnark keeps its own terms hash and spends a
source owned by the order-authority PDA.

## Benchmark

Both implementations measure the same things the same way and write them in
the same format, so their files compare line by line. No file is edited by
hand:

- [`gnark/BENCHMARK.md`](gnark/BENCHMARK.md) and
  [`xark/BENCHMARK.md`](xark/BENCHMARK.md) are written by each
  implementation's `bench_cu` test: circuit sizes, constraint counts, key
  loads and warm proving times, then per instruction the compute units
  (whole instruction, proof verification, SPP CPI), warm circuit and SPP
  proving times and the v1 transaction size. The setup each needs is in the
  doc comment at the top of its `test/tests/bench_cu.rs`.
- [`BENCHMARK.md`](BENCHMARK.md) puts the two side by side with the delta.
  Regenerate it from the repository root after either bench:

```bash
python3 test-utils/compare-benchmarks.py swap
```

## gnark

### Layout

- [`gnark/program/`](gnark/program/) — the Pinocchio program. Verifies a
  Groth16 proof against the public-input hash, then CPIs SPP `transact`.
- [`gnark/prover/`](gnark/prover/) — in-process proving engine on the shared
  `spr-gnark-ffi-prover`. Go gnark circuits, their registration, and the
  key-generation binary.
- [`gnark/sdk/`](gnark/sdk/) — client library. State, instruction and
  proof-input builders, UTXO hashing, discovery, and the prover client.
- [`gnark/test/`](gnark/test/) — localnet end-to-end tests and CU benchmarks
  ([`gnark/BENCHMARK.md`](gnark/BENCHMARK.md)).

### Build

Each crate is its own package with no shared workspace, so build them one at a
time. The Zolana crates come from commit `6bb4f7008294695ebe614f08affb215d4321a538` of
[helius-labs/zolana](https://github.com/helius-labs/zolana). Run the commands
below from `swap/gnark/`.

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --features bpf-entrypoint
cargo build --manifest-path sdk/Cargo.toml
```

The prover compiles Go gnark circuits, so building `prover`, `sdk` or `test`
needs Go 1.27.3 or newer alongside Rust. The circuits import the `github.com/helius-labs/zolana/prover/server`
and `zolana/gnarksdk` Go modules, which
[`gnark/prover/circuits/go.mod`](gnark/prover/circuits/go.mod) takes from a Zolana checkout
named `zolana` next to the `zolana-examples` checkout:

```bash
git clone https://github.com/helius-labs/zolana ../../../zolana
git -C ../../../zolana checkout 6bb4f7008294695ebe614f08affb215d4321a538
```

The Go bridge to Rust ships inside the `spr-gnark-ffi-prover-build` crate
and needs no checkout.

The circuit tests need the pinned proving and verifying keys, whose hashes are
in [`gnark/swap-keys.CHECKSUM`](gnark/swap-keys.CHECKSUM). They are insecure test keys from
a fixed public seed. That is the recommended setup for integration tests, and it
is secure there because a test validator holds nothing a forged proof could
take. They are UNSAFE for production. Generate them into `build/gnark/` with the
`swap-prover-setup` binary:

```bash
for c in make take cancel; do
  cargo run --manifest-path prover/Cargo.toml --bin swap-prover-setup -- \
    "$c" "build/gnark/$c" --insecure-test-keys
done
```

## xark

### Layout

- [`xark/program/`](xark/program/) — the Pinocchio program and the circuits.
  [`src/circuits/`](xark/program/src/circuits/) holds one Rust file per circuit
  (`make`, `take`, `cancel`), the order terms state (`terms.rs`) and the
  shared order spend and blinding seed check (`shared.rs`);
  [`src/instructions/`](xark/program/src/instructions/) verifies the proof
  with the generated verifier in [`src/verifier/`](xark/program/src/verifier/),
  then CPIs SPP `transact`.
- [`xark/sdk/`](xark/sdk/) — client library. Order state, transaction
  builders over the generated circuit clients, instruction builders,
  discovery, and the in-process prover over the checksum-pinned Xark
  artifacts. Circuit tests in `sdk/tests/`.
- [`xark/test/`](xark/test/) — localnet end-to-end tests and the benchmark
  ([`xark/BENCHMARK.md`](xark/BENCHMARK.md)).

The program ID is `F4hjig5RJbXW6vrgYaMcuWzR55kZF6tbKyf8c36nrtvU`.

### Circuits

The circuits use the spr program SDK's circuit API, like the timelock escrow's
Xark implementation. Each circuit is a `#[derive(ZkProgram)]` struct with
`#[derive(ClientInputs)]` private inputs (a `TxContext`, the order terms, the
spent UTXOs) and `#[derive(PublicInputs)]` public inputs. `execute` spends and
creates UTXOs with `TokenUtxos` and `DataUtxo<OrderTerms>`, moves value with
`transfer`, and assembles the transaction with `ConfidentialTransaction`,
whose `check` derives the output blindings and the private transaction hash.
The SDK generates `MakeClient`, `TakeClient` and `CancelClient`; their
`prepare` runs `execute` natively and builds the SPP transaction the proof
commits to.

- **Order terms** (`OrderTerms`): destination mint, destination amount, the
  maker's owner, expiry and `taker_pk_fe` (32 big-endian bytes, zero for an
  open order). The struct derives `UtxoState`, so the circuits only set its
  fields: the SDK hashes the state as `Poseidon(discriminator,
  hash_bytes(destination_mint), destination_amount, maker_owner_hash, expiry,
  hash_bytes(taker_pk_fe))` and encodes it with Borsh. The order UTXO is a
  `DataUtxo<OrderTerms>` owned by the order-authority PDA, so its data hash is
  `Poseidon(DATA, state_hash)` and its encrypted `utxo_data` the Borsh-encoded
  terms, which the taker and the maker decode with `DataUtxo::try_from`
  against the committed leaf. The terms commit the maker's owner, not its
  viewing key: the taker checks the registry-resolved maker address against
  that owner.
- **make** spends the maker's source (`TokenUtxos::new_mut`), whose owner
  must have the public signer identity, creates the order UTXO, moves
  `source_amount` into it and sets the terms with the source owner as the
  maker; the SDK returns the rest to the source owner as change (outputs
  `[change, order]`), with a random blinding seed. The maker signs the
  transact as the source's owner signer.
- **take** spends the order UTXO and the taker's destination input and pays
  the source to the taker and exactly `destination_amount` to the maker
  (outputs `[source, destination]`); its `maker_address` input only names the
  viewing key of the maker payout ciphertext. It requires
  `tx_context.blinding_seed == Poseidon("SWTX", order_utxo.blinding)` and that
  the transaction's first nullifier is the public one, so the maker derives its
  payout from the order opening and the first nullifier. A nonzero
  `taker_pk_fe` binds the taker input's owner:
  `taker_pk_fe * (owner_hash - Poseidon(taker_pk_fe, nullifier_pk)) == 0`.
- **cancel** spends the order UTXO back to the maker, whose signing key
  identity must be the public signer; random blinding seed.

Public inputs fold into the one public input as
`Poseidon(fields..., transaction_hash)`, where `transaction_hash` is the SPP
transaction's `private_tx_hash` (no public transfers). The program recomputes
the fields:

| Circuit | Public fields | Program source |
| ------- | ------------- | -------------- |
| make | `maker_owner_pk_field` | `solana_owner_identity(maker signer)` |
| take | `expiry`, `first_nullifier` | `transact.expiry_unix_ts`, `transact.inputs[0].nullifier_hash` |
| cancel | `expiry`, `maker_owner_pk_field` | `order_expiry`, `solana_owner_identity(maker signer)` |

### Build

One Cargo workspace. Use Rust 1.95.0 (`rust-toolchain.toml`), Agave with SBF
platform tools v1.54, and Go for the localnet services. The workspace pins
Zolana
[`af9d32923`](https://github.com/helius-labs/zolana/commit/af9d32923c047e17c7a5b2985c92492ae18bfa2e)
and Xark
[`aec8530`](https://github.com/ananas-block/xark/commit/aec8530d4526a082daed68ca2dd9c21d47a86f26),
the Xark revision that Zolana revision pins. The setup script checks out the
SDK under `.tools/`, checks that the SDK and this workspace pin the same Xark
revision, and builds the Zolana and Xark CLIs. Run the commands below from
`swap/xark/`.

```bash
scripts/setup-zk-tools.sh
export ZOLANA_CLI_BIN="$PWD/.tools/zolana/target/release/zolana"
"$ZOLANA_CLI_BIN" zk compile --test --skip-wasm --manifest-path program/Cargo.toml
scripts/check-keys.sh
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --locked --features bpf-entrypoint
```

Platform tools v1.54 match the pinned SDK, which builds the shielded-pool
program with the same version.

`zolana zk compile --test` writes each circuit's `circuit.xbc`, `pk.bin` and
`vk.bin` to `target/zk/swap-program/<circuit>/` and its verifier to
`program/src/verifier/`. [`xark/swap-keys.CHECKSUM`](xark/swap-keys.CHECKSUM)
pins the bytecode and keys the SDK proves with. They are insecure test keys
from a fixed public seed, UNSAFE for production. `--check` compares the
committed verifiers with cached keys without rewriting them. After an
intentional circuit change, regenerate, then update the checksums with
`shasum -a 256` over `*/circuit.xbc`, `*/pk.bin` and `*/vk.bin` relative to
`target/zk/swap-program/`. `SWAP_KEYS_DIR` points the SDK and the checksum
script at another directory with the same artifacts.

### Test

```bash
cargo fmt -p swap-program -p swap-sdk -p swap-test -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked -p swap-program --features bpf-entrypoint
cargo test --release --locked -p swap-sdk
```

The end-to-end tests start a localnet with surfpool, Photon, the SPP prover
and the user registry. The release services do not match this SDK revision,
so build them from the SDK checkout with `scripts/build-localnet.sh` (needs
Go), which prints the variables to export (`PROVER_BIN`, `ZOLANA_PHOTON_BIN`,
`SWAP_PHOTON_BIN`, `SHIELDED_POOL_PROGRAM_PATH`, `USER_REGISTRY_PROGRAM_PATH`).
Also set `ZOLANA_CLI_BIN` and `SURFPOOL_BIN` to `surfpool` from
[v1.6.0-light](https://github.com/Lightprotocol/surfpool/releases/tag/v1.6.0-light).
`ZOLANA_PORT_OFFSET` (a multiple of 100) and `ZOLANA_PROVER_URL` move the
ports away from other localnets.

```bash
eval "$(scripts/build-localnet.sh | sed 's/^/export /')"
mkdir -p ~/.config/zolana/proving-keys
cargo test --release --locked -p swap-test
```

The benchmark replays make, take and cancel under mollusk, times warm
circuit and SPP proofs and circuit key loading, and writes
[`xark/BENCHMARK.md`](xark/BENCHMARK.md) in the format the gnark benchmark
shares. It needs `SHIELDED_POOL_PROGRAM_PATH` and starts the SPP prover with
`ZOLANA_CLI_BIN`. Keep its profiled program in `target/bench`, because a
validator does not register the profiler syscall.

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --locked --features bpf-entrypoint,profile-program
cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture
```

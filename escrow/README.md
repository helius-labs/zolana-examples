# Timelock Escrow Program

A timelock escrow on the Solana Privacy Program (SPP): a creator locks a
private balance as a shielded UTXO with an unlock timestamp, and withdraws it
once that timestamp has passed.

The escrow program is an SPP ZK program: it verifies a small proof of its own
escrow rules and delegates the confidential transfer to SPP. It stores no state
and owns no accounts.

See [`timelock_escrow.md`](timelock_escrow.md) for the full design: the privacy
model, escrow terms, instructions, and circuits.

## Implementations

- [`gnark/`](gnark/) — circuits written in Go with gnark, proved in process
  through `spr-gnark-ffi-prover`.
- [`xark/`](xark/) — circuits written in Rust and compiled with Xark through
  Zolana's `ZkProgram` derives, proved in process by the Xark prover. See
  [xark](#xark) below.

Both implement the same instructions, tags, instruction data, errors and escrow
terms. They are separate Cargo workspaces with their own program IDs and share
no code.

## Benchmark

Both implementations measure the same things the same way and write them in
the same format, so their files compare line by line. No file is edited by
hand:

- [`gnark/BENCHMARK.md`](gnark/BENCHMARK.md) and
  [`xark/BENCHMARK.md`](xark/BENCHMARK.md) are written by each
  implementation's `bench_cu` test: circuit sizes, constraint counts, key
  loads and warm proving times, then per instruction the compute units
  (whole instruction, proof verification, SPP CPI), warm circuit and SPP
  proving times and the v1 transaction size.
- [`BENCHMARK.md`](BENCHMARK.md) puts the two side by side with the delta,
  written by `test-utils/compare-benchmarks.py` from the two files.

Each bench profiles its program under Mollusk, so build the profiling program
into `target/bench` first; a validator does not register the profiler
syscall. Both read the shielded-pool program from
`SHIELDED_POOL_PROGRAM_PATH` and use the prover at `ZOLANA_PROVER_URL`, or
start one with `ZOLANA_CLI_BIN`. Each needs the shielded-pool program and
prover of its own SDK: the xark ones from `xark/scripts/build-localnet.sh`,
the gnark ones built from Zolana `6aba4a7` as in
`test-utils/run-release-tests.sh`. From `escrow/xark/`, with the circuit
artifacts compiled (see [Build](#build)):

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --locked --features bpf-entrypoint,profile-program
cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture
```

From `escrow/gnark/`, with the circuit keys generated into `build/gnark`:

```bash
for circuit in escrow withdraw; do
    cargo run --release --manifest-path prover/Cargo.toml --bin timelock-escrow-prover-setup -- \
        "$circuit" "$PWD/build/gnark/$circuit" --insecure-test-keys
done
CARGO_TARGET_DIR="$PWD/target" cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
cargo test --release --manifest-path test/Cargo.toml --test bench_cu -- --ignored --nocapture
```

Then, from the repository root:

```bash
python3 test-utils/compare-benchmarks.py escrow
```

## xark

The circuits live in the program crate as Rust. Each circuit's `execute`
method runs natively on the client, where it builds the SPP transaction, and
compiles through Xark for proving. `zolana zk compile` generates the on-chain
verifier of each circuit from its verifying key.

The workspace uses Zolana
[`af9d32923`](https://github.com/helius-labs/zolana/commit/af9d32923c047e17c7a5b2985c92492ae18bfa2e)
and Xark
[`aec8530`](https://github.com/ananas-block/xark/commit/aec8530d4526a082daed68ca2dd9c21d47a86f26),
the Xark revision that Zolana revision pins. Cargo pins both to Git revisions;
this Zolana revision is newer than the published 0.8.0 crates the gnark
implementation uses. The program ID is
`42uWG786CJAdtdZy5KEEtE7QCuSHjVFu5ZAq32Z41hDD`.

What differs from the gnark implementation, within the spec:

- The escrow terms are an SDK state (`UtxoState`): `creator`, the creator's
  owner, and `unlock`. The circuits set them on the escrow `DataUtxo` and read
  them back from it; the SDK hashes them into the data hash
  (`Poseidon(DATA, Poseidon(discriminator, owner_hash, unlock))`) and encrypts
  them as the escrow UTXO's data. The SDK finds the creator's open escrows by
  fetching the transactions tagged with the creator's escrow-authority PDA and
  decrypting them (`state::open_escrows`), and withdraw takes the decoded
  escrow UTXO, so the terms never travel out of band.
- The escrow source is the creator's own UTXO, not one owned by the
  creator's escrow-authority PDA: the creator signs the SPP transact for it,
  and the PDA signs as the owner of the escrow UTXO, which carries data. The
  change is the source's remainder, which the SDK returns to the source owner.
- The proofs' single public input is the SDK's public hash over
  `(owner_pk_field, escrow_owner)` for escrow and `(unlock, owner_pk_field)`
  for withdraw, each with `transaction_hash`, where `owner_pk_field` is
  `solana_owner_identity` of the creator signer, `escrow_owner` is the address
  hash of the creator's escrow-authority PDA owner with the `viewing_pubkey`
  from the instruction data, and `transaction_hash` is `private_tx_hash`
  (neither transaction moves public funds). The program recomputes both from
  its accounts and instruction data. gnark binds only the PDA's owner hash;
  Xark binds the whole escrow owner address, as Zolana's reference escrow
  does.
- The escrow transaction is proved at the SPP `(1, 2)` shape, without a dummy
  input. The output blindings derive from the transaction's first nullifier,
  its blinding seed and the output slot.
- The circuits recompute every spent UTXO's hash from its preimage.

### Layout

- [`xark/program/src/circuits/`](xark/program/src/circuits/) holds the escrow
  and withdraw circuits and the escrow terms state.
- [`xark/program/src/instructions/`](xark/program/src/instructions/) checks the
  accounts, verifies the program proof and invokes SPP `transact`, signed by
  the escrow-authority PDA.
- [`xark/program/src/verifier/`](xark/program/src/verifier/) holds the
  generated verifiers.
- [`xark/sdk/`](xark/sdk/) holds the escrow UTXO lookup, the transaction and
  instruction builders and the checksum-pinned prover.
- [`xark/test/`](xark/test/) holds the localnet test and the CU benchmark.
- [`xark/timelock-escrow-keys.CHECKSUM`](xark/timelock-escrow-keys.CHECKSUM)
  pins each circuit's bytecode, proving key and verifying key.

### Build

Use Rust 1.95.0 (pinned in `xark/rust-toolchain.toml`, which fixes the compiler
that generates the checked bytecode), Agave with SBF platform tools v1.54, and
Go for the localnet services. The setup script checks out the SDK under
`xark/.tools/`, checks that the SDK and this workspace pin the same Xark
revision, and builds the Zolana and Xark CLIs. Run from `escrow/xark/`:

```bash
scripts/setup-zk-tools.sh
export ZOLANA_CLI_BIN="$PWD/.tools/zolana/target/release/zolana"
"$ZOLANA_CLI_BIN" zk compile --test --skip-wasm --manifest-path program/Cargo.toml
scripts/check-keys.sh
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 -- --locked --features bpf-entrypoint
```

`zolana zk compile --test` writes each circuit's `circuit.xbc`, `pk.bin` and
`vk.bin` to `target/zk/timelock-escrow-program/<circuit>/` and its verifier to
`program/src/verifier/`. These are deterministic keys from an insecure test
setup, so the program compiles only with its default `insecure-test-setup`
feature; use a trusted setup before deploying it. Check that the circuits
reproduce the committed verifiers and checksums with
`"$ZOLANA_CLI_BIN" zk compile --test --manifest-path program/Cargo.toml --check`
and `scripts/check-keys.sh`. After an intentional circuit change, regenerate
them and update the checksums with `shasum -a 256` over the generated files,
relative to `target/zk/timelock-escrow-program/`. `TIMELOCK_ESCROW_KEYS_DIR`
points the SDK and the checksum script at another directory with the same
artifacts.

### Test

```bash
cargo fmt -p timelock-escrow-program -p timelock-escrow-sdk -p timelock-escrow-test -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked -p timelock-escrow-program --features bpf-entrypoint
cargo test --release --locked -p timelock-escrow-sdk
```

The program tests run the SBF build under Mollusk, so build it first. The SDK
tests prove both circuits with the checked artifacts and check that the
compiled circuits refuse broken inputs.

The localnet test starts surfpool, Photon and the SPP prover. The release
binaries do not match this SDK revision, so build the prover, Photon and the
shielded-pool program from the SDK checkout with `scripts/build-localnet.sh`,
which prints the variables to export (`PROVER_BIN`, `ZOLANA_PHOTON_BIN`,
`TIMELOCK_ESCROW_PHOTON_BIN`, `SHIELDED_POOL_PROGRAM_PATH`). Set `SURFPOOL_BIN`
to `surfpool` from
[v1.6.0-light](https://github.com/Lightprotocol/surfpool/releases/tag/v1.6.0-light).
The prover downloads missing SPP keys to `ZOLANA_PROVER_KEYS_DIR`, or
`~/.config/zolana/proving-keys` by default. `ZOLANA_PORT_OFFSET` and
`ZOLANA_PROVER_URL` move the services off the default ports.

```bash
eval "$(scripts/build-localnet.sh | sed 's/^/export /')"
mkdir -p ~/.config/zolana/proving-keys
cargo test --release --locked -p timelock-escrow-test --test escrow
```

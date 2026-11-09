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
  through `spr-gnark-ffi-prover`. CU numbers are in
  [`gnark/BENCHMARK.md`](gnark/BENCHMARK.md).
- [`xark/`](xark/) — circuits written in Rust and compiled with Xark through
  Zolana's `ZkProgram` derives, proved in process by the Xark prover. CU numbers
  are in [`xark/BENCHMARK.md`](xark/BENCHMARK.md). See [xark](#xark) below.

Both implement the same instructions, tags, instruction data, errors and escrow
terms. They are separate Cargo workspaces with their own program IDs and share
no code.

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

- The proofs' single public input is the SDK's public hash:
  `Poseidon(transaction_hash)` for escrow and
  `Poseidon(unlock, owner_pk_field, transaction_hash)` for withdraw, where
  `transaction_hash` is `private_tx_hash` (neither transaction moves public
  funds). The program recomputes both from its accounts and instruction data.
- The escrow transaction is proved at the SPP `(1, 2)` shape, without a dummy
  input. The output blindings derive from the transaction's first nullifier,
  its blinding seed and the output slot.
- The circuits recompute every spent UTXO's hash from its preimage.

### Layout

- [`xark/program/src/circuits/`](xark/program/src/circuits/) holds the escrow
  and withdraw circuits and the escrow terms.
- [`xark/program/src/instructions/`](xark/program/src/instructions/) checks the
  accounts, verifies the program proof and invokes SPP `transact`, signed by
  the escrow-authority PDA.
- [`xark/program/src/verifier/`](xark/program/src/verifier/) holds the
  generated verifiers.
- [`xark/sdk/`](xark/sdk/) holds the escrow state, the transaction and
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

### Benchmark

The benchmark profiles both instructions under Mollusk and writes
[`xark/BENCHMARK.md`](xark/BENCHMARK.md). It reads the SPP binary from
`SHIELDED_POOL_PROGRAM_PATH` and uses the prover at `ZOLANA_PROVER_URL`, or
starts one with `ZOLANA_CLI_BIN`. Keep its profiled program in `target/bench`,
because a validator does not register the profiler syscall.

```bash
cargo build-sbf --manifest-path program/Cargo.toml --tools-version v1.54 --sbf-out-dir target/bench -- --features bpf-entrypoint,profile-program
cargo test --release --locked -p timelock-escrow-test --test bench_cu -- --ignored --nocapture
```

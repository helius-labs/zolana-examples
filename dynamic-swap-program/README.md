# Dynamic swap

Create a pair, update its price, and settle or refund an escrow.

Copied from [`sdk-tests/dynamic-swap`](https://github.com/helius-labs/zolana/tree/ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856/sdk-tests/dynamic-swap) at `ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856`.

## Setup

Install Rust 1.98.1, Go 1.27.1 (or Go with automatic toolchain downloads), `just`, `gh`, Clang/libclang, and `lsof`. Authenticate `gh` to download the release artifacts.

From the repository root:

```bash
scripts/program-examples.sh prepare
```

Setup prepares the pinned Zolana stack, Surfpool 1.6.0, and Agave 4.2.2 in `.cache/`, builds the programs, and downloads the matching proving keys. It leaves your installed Solana tools unchanged. The first build and proof-key downloads can take several minutes.

## Run

Stop other local validators, Photon, and prover processes first. The tests use local ports 8899, 8900, 8784, 3001, and 9998 and fund fresh wallets on localnet.

```bash
scripts/program-examples.sh test dynamic-swap
```

The command runs the original tests and stops the local services afterward. Run `scripts/program-examples.sh check` to check formatting and all Rust targets for all three imported examples.

## Benchmark

```bash
scripts/program-examples.sh bench dynamic-swap
```

At the pinned source revision, this ignored benchmark fails while proving its fixture (`constraint #45150 is not satisfied`). The same failure reproduces in the unchanged Zolana source. The regular localnet and circuit tests pass; the benchmark fixture is preserved here.

`BENCHMARK.md` retains the upstream measurement.

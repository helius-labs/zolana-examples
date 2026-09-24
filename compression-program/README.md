# Compression

Create and update plaintext compressed accounts.

Copied from [`sdk-tests/compression`](https://github.com/helius-labs/zolana/tree/ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856/sdk-tests/compression) at `ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856`.

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
scripts/program-examples.sh test compression
```

The command runs the original tests and stops the local services afterward. Run `scripts/program-examples.sh check` to check formatting and all Rust targets for all three imported examples.

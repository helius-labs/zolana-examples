# Zolana examples

Examples for Zolana private balances and transactions, from client flows that
drive the `zolana-client` SDK to ZK programs built on the Solana Privacy
Program (SPP).

## Client examples

Self-contained Rust binaries that drive the `zolana-client` SDK against a local
validator, Photon indexer, and prover. They live in
[`rust-client/`](rust-client/) — see its [README](rust-client/README.md) to
build the prerequisites and run them.

- **create_private_wallet** — create and register a wallet for a private balance.
- **deposit** — move public tokens into a private balance.
- **transfer** — send privately between two private balances.
- **withdraw** — move a private balance back to a public account.
- **sync_balance** — read a wallet's private balance from the indexer.

## Program examples

- [`swap-program/`](swap-program/) — a confidential swap between a maker and a
  taker on SPP. An SPP ZK program that verifies a proof of its own swap rules
  and delegates the confidential transfer to SPP.

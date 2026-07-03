# Zolana examples

Runnable client examples for Zolana private balances and transactions. Each
example is a self-contained Rust binary that drives the `zolana-client` SDK
against a local validator, Photon indexer, and prover.

The examples live in [`rust-client/`](rust-client/) — see its
[README](rust-client/README.md) to build the prerequisites and run them.

## Examples

- **deposit** — move public tokens into a private balance.
- **transfer** — send privately between two private balances.
- **withdraw** — move a private balance back to a public account.
- **sync_balance** — read a wallet's private balance from the indexer.

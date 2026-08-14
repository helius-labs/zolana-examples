# Zolana examples: Rust client

|  |  |
|---------|-------------|
| [`deposit`](examples/deposit_instruction.rs) | Move SOL from a public to a private balance. |
| [`transfer`](examples/transfer_instruction.rs) | Transfer SOL between private balances. |
| [`withdraw`](examples/withdraw_instruction.rs) | Move SOL from a private to a public balance. |

## Setup

Copy the env template and set your [Helius API key](https://dashboard.helius.dev/):

```bash
cp .env.example .env
```

By default, the examples use your CLI wallet as `payer`. Make sure it's funded with [devnet SOL](https://faucet.solana.com/).

To run on localnet, toggle `localnet` in [`src/lib.rs`](src/lib.rs).

## Run

```bash
cargo run -p rust-client-example --example deposit_instruction
cargo run -p rust-client-example --example transfer_instruction
cargo run -p rust-client-example --example withdraw_instruction
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

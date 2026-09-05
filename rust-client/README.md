# Zolana examples: Rust client

|  |  |
|---------|-------------|
| [`create_private_wallet`](examples/create_private_wallet.rs) | Create and register a private wallet. |
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |

## Setup

Copy the env template and set your [Helius API key](https://dashboard.helius.dev/):

```bash
cp .env.example .env
```

By default, the examples use your CLI wallet as `payer`. Make sure it's funded with [devnet SOL](https://faucet.solana.com/).

To run on localnet, toggle `localnet` in [`src/lib.rs`](src/lib.rs).

## Run

```bash
cargo run -p rust-client-example --example create_private_wallet
cargo run -p rust-client-example --example deposit_transfer_withdraw
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

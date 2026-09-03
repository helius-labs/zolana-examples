# Zolana examples: Rust client

|  |  |
|---------|-------------|
| [`create_private_wallet`](examples/create_private_wallet.rs) | Create a private wallet and register its Solana address. |
| [`create_private_wallet_spl`](examples/create_private_wallet_spl.rs) | Create a private wallet and register its Solana address. |
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |
| [`sync_balance`](examples/sync_balance.rs) | Read the private SOL and SPL balances. |
| [`read_history`](examples/read_history.rs) | Read the private transaction history. |

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
cargo run -p rust-client-example --example create_private_wallet_spl
cargo run -p rust-client-example --example deposit_transfer_withdraw
cargo run -p rust-client-example --example sync_balance
cargo run -p rust-client-example --example read_history
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

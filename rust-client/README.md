# Zolana examples: Rust client

|  |  |
|---------|-------------|
| [`register_private_wallet`](examples/register_private_wallet.rs) | Register a private wallet. |
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.rs) | Deposit, private transfer, and withdraw. |
| [`deposit_with_interface_setup`](examples/deposit_with_interface_setup.rs) | Create a token interface and deposit in one transaction. |
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
cargo run -p rust-client-example --example register_private_wallet
cargo run -p rust-client-example --example deposit_transfer_withdraw
cargo run -p rust-client-example --example deposit_with_interface_setup
cargo run -p rust-client-example --example sync_balance
cargo run -p rust-client-example --example read_history
```

`deposit_with_interface_setup` prepares a fresh test token, then checks that its interface PDA is absent. One transaction creates the mint registry PDA and token vault, then deposits into the sender's private balance. The sender pays transaction fees and account rent. The network must allow permissionless interface creation.

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

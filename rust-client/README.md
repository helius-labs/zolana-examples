# Zolana Examples - Rust Client

|  |  |  |  |
|---------|-------------|---------|---------|
| [`transfer`](examples/transfer.rs) | Transfer between private balances. | [Action](examples/transfer.rs) | [Instruction](examples/transfer_instruction.rs) |
| [`deposit`](examples/deposit.rs) | Move tokens from a public to a private balance. | [Action](examples/deposit.rs) | [Instruction](examples/deposit_instruction.rs) |
| [`withdraw`](examples/withdraw.rs) | Move tokens from a private to a public balance. | [Action](examples/withdraw.rs) | [Instruction](examples/withdraw_instruction.rs) |
| [`create_private_wallet`](examples/create_private_wallet.rs) | Create a private wallet. | [Action](examples/create_private_wallet.rs) |  |
| [`sync_balance`](examples/sync_balance.rs) | Read a wallet's private balance. | [Action](examples/sync_balance.rs) |  |

## Setup

Copy the env template and set your [Helius API key](https://dashboard.helius.dev/):

```bash
cp .env.example .env
```

By default, the examples use your CLI wallet as `payer`. Make sure it's funded with [devnet SOL](https://faucet.solana.com/).

To run on localnet, toggle `localnet` in [`src/lib.rs`](src/lib.rs).

## Run

```bash
cargo run -p rust-client-example --example transfer
cargo run -p rust-client-example --example deposit
cargo run -p rust-client-example --example withdraw
cargo run -p rust-client-example --example create_private_wallet
cargo run -p rust-client-example --example sync_balance

cargo run -p rust-client-example --example transfer_instruction
cargo run -p rust-client-example --example deposit_instruction
cargo run -p rust-client-example --example withdraw_instruction
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

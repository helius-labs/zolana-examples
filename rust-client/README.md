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

### Localnet

Requirements:

- zolana cli (install via `cargo install --git https://github.com/helius-labs/zolana --tag v0.1.0-alpha zolana-cli`)
- solana cli version 4.x

Start the localnet and fund the payer:

```bash
zolana test-env --with-photon --no-use-surfpool
solana airdrop 100 --url http://127.0.0.1:8899
```

NOTE: `--no-use-surfpool` runs the solana test validator directly; the default
surfpool backend rejects account creation on localnet.

Point the examples at localnet in [`src/lib.rs`](src/lib.rs): uncomment the
three `// localnet:` URLs and remove the devnet ones.

`zolana test-env` spawns the following background processes:

1. solana test validator `http://127.0.0.1:8899`
2. prover server `http://127.0.0.1:3001`
3. photon indexer `http://127.0.0.1:8784`

Stop them with `zolana test-env --stop`.

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

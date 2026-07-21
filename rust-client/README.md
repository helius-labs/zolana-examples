# Zolana - Rust Client

| Example | Description |
|---------|-------------|
| [`create_private_wallet`](examples/create_private_wallet.rs) | Create and register a wallet for a private balance. |
| [`deposit`](examples/deposit.rs) | Move public tokens into a private balance. |
| [`transfer`](examples/transfer.rs) | Send a value privately between two private balances. |
| [`withdraw`](examples/withdraw.rs) | Withdraw a private balance back to a public account. |
| [`sync_balance`](examples/sync_balance.rs) | Read a wallet's private balance from the indexer. |

## Prerequisites

The examples run against a local stack: a Solana validator, a Photon indexer,
and the Zolana prover. Start all three from a [zolana](https://github.com/helius-labs/zolana)
checkout (see its `justfile`); the defaults are:

| Service | URL |
|---------|-----|
| Validator RPC | `http://127.0.0.1:8899` |
| Photon indexer | `http://127.0.0.1:8784` |
| Prover | `http://127.0.0.1:3001` |

The zolana SDK crates are private git dependencies, so building needs an
SSO-authorized GitHub SSH key. `.cargo/config.toml` sets `git-fetch-with-cli`
so cargo fetches them over that key.

## Configure

Copy `.env.example` to `.env`. The payer defaults to the Solana CLI wallet
(`~/.config/solana/id.json`); override it and the service URLs only if your
setup differs from the defaults above.

```bash
cp .env.example .env
```

## Run

With the local stack up and `.env` in place:

```bash
cargo run -p rust-client-example --example deposit
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
- [AI Skill](https://example.com/zolana-ai-skill)

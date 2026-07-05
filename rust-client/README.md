# Zolana - Rust Client

| Example | Description |
|---------|-------------|
| [`create_private_wallet`](examples/create_private_wallet.rs) | Create and register a wallet for a private balance. |
| [`deposit`](examples/deposit.rs) | Move public tokens into a private balance. |
| [`transfer`](examples/transfer.rs) | Send a value privately between two private balances. |
| [`withdraw`](examples/withdraw.rs) | Withdraw a private balance back to a public account. |
| [`sync_balance`](examples/sync_balance.rs) | Read a wallet's private balance from the indexer. |

## Configure

Copy `.env.example` to `.env` and set `API_KEY`. The tree default is prefilled and
the payer defaults to `~/.config/solana/id.json`, so a Helius key is all you need
to add:

```bash
cp .env.example .env
```

## Run

With `.env` in place:

```bash
cargo run -p rust-client-example --example deposit
```

Values set inline on the command still work and take precedence over `.env`:

```bash
API_KEY=<helius key> ZOLANA_TREE=<tree> ZOLANA_PAYER_KEYPAIR=<funded key> \
  cargo run -p rust-client-example --example deposit
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)
- [AI Skill](https://example.com/zolana-ai-skill)

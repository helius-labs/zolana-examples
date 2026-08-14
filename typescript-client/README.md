# Zolana examples: TypeScript client

|                                                                      |                                          |
| -------------------------------------------------------------------- | ---------------------------------------- |
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.ts) | Deposit, private transfer, and withdraw. |

## Setup

Install Node.js 24+ and pnpm. The TypeScript SDK is pinned directly to the
Zolana git revision used by the Rust examples.

```bash
pnpm install
cp .env.example .env
```

`ZOLANA_PAYER_KEYPAIR` defaults to the Solana CLI wallet. It must be funded on
the selected network.

With no `ZOLANA_ENDPOINT`, the SDK uses its local validator, Photon, and prover
defaults. For Helius devnet RPC plus a separate Photon/prover host, set
`ZOLANA_ENDPOINT` to the RPC URL and `ZOLANA_INDEXER_URL` / `ZOLANA_PROVER_URL`
to those services. Hosted indexer and prover URLs must be HTTPS; loopback
`http://127.0.0.1` is allowed.

## Run

```bash
pnpm example examples/deposit_transfer_withdraw.ts
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

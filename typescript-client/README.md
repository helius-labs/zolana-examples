# Zolana examples: TypeScript client

|                                                |                                              |
| ---------------------------------------------- | -------------------------------------------- |
| [`deposit`](examples/deposit_instruction.ts)   | Move SOL from a public to a private balance. |
| [`transfer`](examples/transfer_instruction.ts) | Transfer between private balances.           |
| [`withdraw`](examples/withdraw_instruction.ts) | Move SOL from a private to a public balance. |

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
pnpm example examples/deposit_instruction.ts
pnpm example examples/transfer_instruction.ts
pnpm example examples/withdraw_instruction.ts
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

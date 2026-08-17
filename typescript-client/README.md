# Zolana examples: TypeScript client

|                                                                      |                                          |
| -------------------------------------------------------------------- | ---------------------------------------- |
| [`deposit_transfer_withdraw`](examples/deposit_transfer_withdraw.ts) | Deposit, private transfer, and withdraw. |

## Setup

Install Node.js 24+ and pnpm.

```bash
pnpm install
cp .env.example .env
```

`ZOLANA_PAYER_KEYPAIR` defaults to the Solana CLI wallet. It must be funded on
the selected network. Set `API_KEY` for the Helius RPC. The example talks to
Helius plus the Photon/prover ALB unless you override `ZOLANA_ENDPOINT` /
`ZOLANA_INDEXER_URL` / `ZOLANA_PROVER_URL`.

To run on localnet, toggle `localnet` in [`src/lib.ts`](src/lib.ts).

## Run

```bash
pnpm example examples/deposit_transfer_withdraw.ts
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

# Zolana examples: TypeScript client

The examples show the same SDK flows at two levels:

|                                                              |                                              |                                             |                                                 |
| ------------------------------------------------------------ | -------------------------------------------- | ------------------------------------------- | ----------------------------------------------- |
| [`transfer`](examples/transfer.ts)                           | Transfer between private balances.           | [Action](examples/transfer.ts)              | [Instruction](examples/transfer_instruction.ts) |
| [`deposit`](examples/deposit.ts)                             | Move SOL from a public to a private balance. | [Action](examples/deposit.ts)               | [Instruction](examples/deposit_instruction.ts)  |
| [`withdraw`](examples/withdraw.ts)                           | Move SOL from a private to a public balance. | [Action](examples/withdraw.ts)              | [Instruction](examples/withdraw_instruction.ts) |
| [`create_private_wallet`](examples/create_private_wallet.ts) | Create and register a private wallet.        | [Action](examples/create_private_wallet.ts) |                                                 |
| [`sync_balance`](examples/sync_balance.ts)                   | Read a wallet's private balance.             | [Action](examples/sync_balance.ts)          |                                                 |

The action examples use `buildDepositTransaction`,
`buildTransferTransaction`, and `buildWithdrawalTransaction`. These builders
produce unsigned Kit transactions; the application signs and submits them. The
instruction examples keep UTXO selection, proof construction, instruction
assembly, submission, and balance decryption visible.

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
`http://127.0.0.1` is allowed. Plain HTTP to a non-loopback host is accepted
only when the examples set `allowInsecureHttp` for that case.
`ZOLANA_RECIPIENT` must identify a registered private wallet for the
action-level transfer.

## Run

```bash
pnpm example examples/transfer.ts
pnpm example examples/deposit.ts
pnpm example examples/withdraw.ts
pnpm example examples/create_private_wallet.ts
pnpm example examples/sync_balance.ts

pnpm example examples/transfer_instruction.ts
pnpm example examples/deposit_instruction.ts
pnpm example examples/withdraw_instruction.ts
```

## Documentation

- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

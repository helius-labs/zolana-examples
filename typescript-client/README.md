# Zolana examples: TypeScript client

TypeScript client examples for `@heliuslabs/zolana`.

- **[register_private_wallet](examples/register_private_wallet.ts)** - Register a private wallet
- **[deposit_transfer_withdraw](examples/deposit_transfer_withdraw.ts)** - Deposit, private transfer, and withdraw
- **[sync_balance](examples/sync_balance.ts)** - Read the private SOL and SPL balances
- **[read_history](examples/read_history.ts)** - Read the private transaction history

## Setup

Install Node.js 24+ and pnpm.

```bash
pnpm install --frozen-lockfile
```

**Devnet:**

The example uses devnet by default.

Get an API key from [Helius](https://helius.dev) and add to .env:

```bash
cp .env.example .env # ...and set API_KEY
```

**Localnet**:

To run on localnet, build the programs, Photon, and prover from Zolana revision
`ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856` using the
[source checkout workflow](https://github.com/helius-labs/zolana/blob/ebca3ad1bd2f27b1f1f04e33fbfa3cc4c7bbf856/cli/README.md#local-dev-environment)
with `zolana dev start --local`. Configure [`src/lib.ts`](src/lib.ts):

```typescript
const RPC_URL = "http://127.0.0.1:8899";
const INDEXER_URL = "http://127.0.0.1:8784";
const PROVER_URL = "http://127.0.0.1:3001";
```

## Run

```bash
pnpm example examples/register_private_wallet.ts
pnpm example examples/deposit_transfer_withdraw.ts
pnpm example examples/sync_balance.ts
pnpm example examples/read_history.ts
```

## Documentation

- [Connect](https://www.helius.dev/docs/privacy/connect)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

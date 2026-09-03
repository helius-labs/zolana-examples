# Zolana examples: TypeScript client

TypeScript client examples for `@heliuslabs/zolana`.

- **[create_private_wallet](examples/create_private_wallet.ts)** - Create a private wallet and register its Solana address
- **[create_private_wallet_spl](examples/create_private_wallet_spl.ts)** - Create a private wallet and register its Solana address
- **[deposit_transfer_withdraw](examples/deposit_transfer_withdraw.ts)** - Deposit, private transfer, and withdraw
- **[sync_balance](examples/sync_balance.ts)** - Read the private SOL and SPL balances
- **[read_history](examples/read_history.ts)** - Read the private transaction history

## Setup

Install Node.js 24+ and pnpm.

```bash
npm install @heliuslabs/zolana@alpha @solana/kit
pnpm install
```

**Devnet:**

The example uses devnet by default.

Get an API key from [Helius](https://helius.dev) and add to .env:

```bash
cp .env.example .env # ...and set API_KEY
```

**Localnet**:

To run on localnet, configure in [`src/lib.ts`](src/lib.ts) and install:

```bash
cargo install --git https://github.com/helius-labs/zolana --tag v0.1.0-alpha zolana-cli
zolana dev start
```

```typescript
const RPC_URL = "http://127.0.0.1:8899";
const INDEXER_URL = "http://127.0.0.1:8784";
const PROVER_URL = "http://127.0.0.1:3001";
```

## Run

```bash
pnpm example examples/create_private_wallet.ts
pnpm example examples/create_private_wallet_spl.ts
pnpm example examples/deposit_transfer_withdraw.ts
pnpm example examples/sync_balance.ts
pnpm example examples/read_history.ts
```

## Documentation

- [Connect](https://www.helius.dev/docs/privacy/connect)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

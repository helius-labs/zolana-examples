# Zolana examples: TypeScript client

TypeScript client examples for `@heliuslabs/zolana`.

- **[deposit_transfer_withdraw](examples/deposit_transfer_withdraw.ts)** - Deposit, private transfer, and withdraw

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
pnpm example examples/deposit_transfer_withdraw.ts
```

## Documentation

- [Connect](https://www.helius.dev/docs/privacy/connect)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

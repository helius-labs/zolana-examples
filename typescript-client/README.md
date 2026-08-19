# Zolana examples: TypeScript client

TypeScript client examples for `@heliuslabs/zolana`.

- **[deposit_transfer_withdraw](examples/deposit_transfer_withdraw.ts)** - Deposit, private transfer, and withdraw

## Setup

Install Node.js 24+ and pnpm.

```bash
npm install @heliuslabs/zolana@alpha @solana/kit
pnpm install
```

For Devnet:

```bash
cp .env.example .env # ...and set API_KEY
```

```ts
import { createZolanaClient } from "@heliuslabs/zolana";

const client = await createZolanaClient({
  solanaRpcUrl: "https://devnet.helius-rpc.com/?api-key=YOUR_API_KEY",
  indexerUrl: "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com",
  proverUrl:
    "http://zolnet-devnet-1779374825.eu-north-1.elb.amazonaws.com:3001",
  allowInsecureHttp: true,
});
```

For Localnet:

```bash
cargo install --git https://github.com/helius-labs/zolana --tag v0.1.0-alpha zolana-cli
zolana dev start
```

```ts
import { createZolanaClient } from "@heliuslabs/zolana";

const client = await createZolanaClient({});
```

## Run

```bash
pnpm example examples/deposit_transfer_withdraw.ts
```

## Documentation

- [Connect](https://www.helius.dev/docs/privacy/connect)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

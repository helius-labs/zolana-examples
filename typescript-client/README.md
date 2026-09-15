# Zolana examples: TypeScript client

TypeScript client examples for `@heliuslabs/zolana`.

- **[deposit_transfer_withdraw](examples/deposit_transfer_withdraw.ts)** - Deposit, private transfer, and withdraw
- **[register_wallet_with_merge](examples/register_wallet_with_merge.ts)** - Register a wallet, merge notes, and recover from a stale two-device merge

## Setup

Install Node.js 24+ and pnpm.

```bash
pnpm install --frozen-lockfile
```

The SDK is pinned to a packaged build of [PR #317](https://github.com/helius-labs/zolana/pull/317), rebased onto `0.1.6-alpha`. It includes the public registry instruction builders used by `register_wallet_with_merge`. See [package provenance](vendor/README.md).

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
pnpm example examples/register_wallet_with_merge.ts
```

### What `register_wallet_with_merge` shows

1. Registers the wallet and enables merging in one transaction.
2. Deposits three `0.1 SOL` notes and syncs two devices to the same wallet state.
3. Device A merges two notes, leaving the wallet with two notes and the same `0.3 SOL` balance.
4. Device B submits a merge built from its stale state. The request is rejected because Device A already spent the shared input nullifiers.
5. Device B syncs from Device A's confirmed slot, rebuilds the merge, and submits it with fresh Merkle proofs and current `rootIndex` values.
6. The retry succeeds, leaving the original `0.3 SOL` balance consolidated into one note.

The example runs against devnet and checks each balance and note-count transition.

## Documentation

- [Connect](https://www.helius.dev/docs/privacy/connect)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

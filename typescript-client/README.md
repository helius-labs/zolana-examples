# Zolana examples: TypeScript client

TypeScript client examples for `@heliuslabs/zolana`.

- **[register_private_wallet](examples/register_private_wallet.ts)** - Register a private wallet
- **[deposit_transfer_withdraw](examples/deposit_transfer_withdraw.ts)** - Deposit, private transfer, and withdraw
- **[deposit_with_interface_setup](examples/deposit_with_interface_setup.ts)** - Create a token interface and deposit in one transaction
- **[sync_balance](examples/sync_balance.ts)** - Read the private SOL and SPL balances
- **[read_history](examples/read_history.ts)** - Read the private transaction history

## Setup

Install Node.js 24+ and pnpm. The SDK is pinned to the `v0.5.0-alpha` Git tag
until the npm release is published. Installation builds the SDK from that tag.

```bash
pnpm install
```

**Devnet:**

The example uses devnet by default.

Devnet proofs use the attested TEE prover, which fetches Merkle proofs from Photon and assembles the witness server-side. Photon also serves balance and history reads.

Get an API key from [Helius](https://helius.dev) and add to .env:

```bash
cp .env.example .env # ...and set API_KEY
```

**Localnet**:

To run on localnet, configure in [`src/lib.ts`](src/lib.ts) and install:

```bash
cargo install --git https://github.com/helius-labs/zolana --tag v0.5.0-alpha zolana-cli
zolana dev start
```

```typescript
const RPC_URL = "http://127.0.0.1:8899";
const INDEXER_URL = "http://127.0.0.1:8784";
const PROVER_URL = "http://127.0.0.1:3001";
```

For a local prover without an indexer, set `ZOLANA_PROOF_DATA_SOURCE=client`. The localnet integration tests use this mode because they share one prover across separate validator/indexer stacks.

## Run

```bash
pnpm example examples/register_private_wallet.ts
pnpm example examples/deposit_transfer_withdraw.ts
pnpm example examples/deposit_with_interface_setup.ts
pnpm example examples/sync_balance.ts
pnpm example examples/read_history.ts
```

`deposit_with_interface_setup` prepares a fresh test token, then checks that its interface PDA is absent. One transaction creates the mint registry PDA and token vault, then deposits into the sender's private balance. The sender pays transaction fees and account rent. The network must allow permissionless interface creation.

## Documentation

- [Connect](https://www.helius.dev/docs/privacy/connect)
- [Documentation](https://helius.dev/docs/privacy)
- [Source Code](https://github.com/helius-labs/zolana)

# Private wallet Node.js starter

An Express server that creates and runs Solana private wallets with only a
Helius API key. Each wallet is a Solana wallet held by Turnkey in your Helius
project. Its private keys, which read and spend the private balance, are
derived and held by the Helius enclave (Turnkey Verifiable Compute), never by
your server.

Devnet only.

## Getting started

### 1. Install

Install Node.js 24+ and pnpm.

```bash
pnpm install
```

### 2. Configure

```bash
cp .env.example .env # ...and set HELIUS_API_KEY
```

The key's project needs the Wallet-as-a-Service add-on, as embedded wallets
do. Without it, creating a wallet fails with `403`.

### 3. Run

```bash
pnpm dev
```

The server listens on [http://localhost:3300](http://localhost:3300).

## Try it

```bash
# Create a wallet, then fund its address with devnet SOL (https://faucet.solana.com).
curl -X POST http://localhost:3300/wallets

# Move 0.01 SOL into the private balance.
curl -X POST http://localhost:3300/wallets/<address>/deposit \
  -H "Content-Type: application/json" -d '{"lamports": "10000000"}'

# Send 0.003 SOL privately to another wallet that has made a deposit.
curl -X POST http://localhost:3300/wallets/<address>/transfer \
  -H "Content-Type: application/json" -d '{"recipient": "<address>", "lamports": "3000000"}'

# Move 0.003 SOL back to the public balance.
curl -X POST http://localhost:3300/wallets/<address>/withdraw \
  -H "Content-Type: application/json" -d '{"lamports": "3000000"}'

# Public and private balances.
curl http://localhost:3300/wallets/<address>
```

| Endpoint                          | What it reveals on chain                     |
| --------------------------------- | -------------------------------------------- |
| `POST /wallets/:address/deposit`  | Sender, recipient, asset and amount          |
| `POST /wallets/:address/transfer` | Sender and recipient; not asset or amount    |
| `POST /wallets/:address/withdraw` | Sender, recipient, asset and amount          |

## How it works

### 1. Create a wallet

[`src/lib/private-wallet.ts`](./src/lib/private-wallet.ts) `createPrivateWallet`:

1. Creates a Turnkey sub-organization in your Helius project with a Solana
   wallet. Its root user is a P-256 API key the server generates
   ([`src/lib/turnkey.ts`](./src/lib/turnkey.ts) `createSubOrganization`).
2. Verifies the enclave against the release it pins
   ([`src/lib/release.json`](./src/lib/release.json)), then lets the
   enclave's Turnkey key request one signature from the wallet: the
   derivation message of its private keys.
3. Enrolls the server's client key with the private-wallet API, with the
   wallet owner's signature. The API answers with a descriptor that lets the
   key run enclave operations for this wallet.
4. Bootstraps: the enclave asks the wallet to sign the derivation message,
   the server checks and approves exactly that request
   ([`src/lib/bootstrap-approval.ts`](./src/lib/bootstrap-approval.ts)), and
   the enclave derives the private keys and keeps them sealed.

```typescript
const { client, connection } = await enclave(descriptor, clientKey);
const bootstrap = await bootstrapWithApproval(api, expected, (signal) =>
  client.bootstrap(connection, { signal }),
);
```

### 2. Use it

`TvcKeys` is the Zolana SDK's `WalletKeys`, answered by the enclave. Everything
else is the ordinary [Zolana SDK](https://github.com/helius-labs/zolana), and
Turnkey signs each Solana transaction.

```typescript
const keys = new TvcKeys({ client, connection, identity, sealedSeed });
const wallet = new Wallet({ identity: shieldedAddressOf(identity) });
await syncWallet({ client: zolana, wallet, keys });

const transaction = await buildTransferTransaction({
  client: zolana, wallet, keys, feePayer, recipient, amount,
});
```

## What the server stores

[`src/lib/store.ts`](./src/lib/store.ts) writes one JSON file per wallet under
`.data/wallets/`: the Turnkey owner key and the client key, which are secrets,
and the descriptor, identity and sealed seed, which are not. A real
deployment keeps the secrets in a KMS or secret store.

## Links

- [Documentation](https://helius.dev/docs/privacy)
- [Zolana SDK](https://github.com/helius-labs/zolana)
- [Private wallet enclave](https://github.com/helius-labs/zolana-tvc)

# Embedded private wallet

An Express server for embedded Solana wallets with private balances. Turnkey
holds the wallet's signing key; the Helius enclave derives and holds its private
balance keys. Setup requires a Helius API key with Wallet-as-a-Service enabled.

Devnet only. The server has no authentication: anyone who can reach it can
spend every wallet it holds. Run it locally.

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

### Deployment

This starter uses Zolana `0.5.0-alpha` on **devnet-d**:

| Service            | Endpoint                                                                 |
| ------------------ | ------------------------------------------------------------------------ |
| Solana RPC         | `https://devnet.helius-rpc.com` (your API key)                           |
| Private-wallet API | `https://d2mwa6adbrijsr.cloudfront.net/v1/private-wallet` (your API key) |
| Indexer            | `https://d21d98xh5lgv3b.cloudfront.net`                                  |
| Attested prover    | `https://d1unet9hn0mci0.cloudfront.net`                                  |

The private-wallet gateway verifies enrollment messages for the domain
`beta-devnet.helius-rpc.com`, even when reached through CloudFront. The
signed trust bundle in `src/lib/release.json` pins
`private-wallet-devnet-e91c22e`; it is copied from zolana-tvc
[`cb717a8f7509`](https://github.com/helius-labs/zolana-tvc/blob/cb717a8f750929e8307ffb788729ac29c369032c/apps/privacy-wallet/deploy/privacy-wallet.trust.json),
the same commit pinned for `@zolana/tvc-wallet`.

The SDK fetches Merkle proof data on the client. `TvcKeys` seals the resolved
proof request inside the wallet enclave to the devnet-d prover under its
signed `DEVNET_PROVER_POLICY`; the server relays the encrypted request.

Use fresh wallets for this deployment. Data created by the older devnet-c
starter uses a different viewing-key format and deployment; it cannot be
reused here.

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

# Private transaction history.
curl http://localhost:3300/wallets/<address>/transactions

# Public transfers, as from any Solana wallet: SOL, or an SPL token by its mint.
curl -X POST http://localhost:3300/wallets/<address>/send \
  -H "Content-Type: application/json" -d '{"recipient": "<address>", "lamports": "1000000"}'
curl -X POST http://localhost:3300/wallets/<address>/send \
  -H "Content-Type: application/json" -d '{"recipient": "<address>", "mint": "<mint>", "amount": "250000"}'
```

| Endpoint                             | What it reveals on chain                  |
| ------------------------------------ | ----------------------------------------- |
| `POST /wallets/:address/deposit`     | Sender, recipient, asset and amount       |
| `POST /wallets/:address/transfer`    | Sender and recipient; not asset or amount |
| `POST /wallets/:address/withdraw`    | Sender, recipient, asset and amount       |
| `POST /wallets/:address/send`        | Everything, as on any Solana wallet       |
| `GET /wallets/:address/transactions` | Nothing; the enclave decrypts the history |

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
Turnkey signs each Solana transaction. Public transfers
([`src/lib/solana.ts`](./src/lib/solana.ts)) use the Turnkey wallet alone.

```typescript
const keys = new TvcKeys({
  client,
  connection,
  identity,
  sealedSeed,
  prover: { url: ZOLANA_PROVER_URL, policy: DEVNET_PROVER_POLICY },
});
const wallet = new Wallet({ identity: shieldedAddressOf(identity) });
await syncWallet({ client: zolana, wallet, keys, config: { requireSlot } });

const transaction = await buildTransferTransaction({
  client: zolana,
  wallet,
  keys,
  feePayer,
  recipient,
  amount,
});
```

The indexer can trail the chain by a few slots, so every sync waits for the
slot of the wallet's last transaction (`requireSlot`). Without that, a
transfer right after a deposit could miss the new note, or try to spend one
the previous transaction already spent.

## What the server stores

[`src/lib/store.ts`](./src/lib/store.ts) writes one JSON file per wallet under
`.data/wallets/`: the Turnkey owner key and the client key, which are secrets,
and the descriptor, identity, sealed seed and last slot, which are not. A real
deployment keeps the secrets in a KMS or secret store.

## Check changes

```bash
pnpm install --frozen-lockfile
pnpm check
```

For live tests, set `DEVNET_PAYER_KEYPAIR_PATH` in `.env` to a devnet-only
Solana JSON keypair with at least **0.2 SOL**, then run:

```bash
pnpm test:devnet
```

The suite creates two wallets and checks deposits, private transfer, withdrawal,
public SOL/SPL/Token-2022 sends, balances, history, rejected requests and reopening
after restart. Token tests cover recipient account creation and reuse; Token-2022
mints have no extensions. A preload checks encrypted prover requests/responses
and records metadata without headers, queries or bodies.

The runner checks devnet before funding and stops its server on exit or Ctrl-C.
Each run keeps `results.json`, `prover-audit.jsonl`, a redacted `server.log`, and
wallet credentials under gitignored `.data/devnet-e2e/run-*`. Keep that directory
private. The payer sends 0.07 SOL to each wallet and pays setup rent and fees;
funds and accounts are retained after the test.

`.env*` and `.data/` are gitignored; `.env.example` contains placeholders only.
Environment variables override `.env`. Live tests run only when requested;
`pnpm check` and PR CI check formatting and types.

## Links

- [Documentation](https://helius.dev/docs/privacy)
- [Zolana SDK](https://github.com/helius-labs/zolana)
- [Private wallet enclave](https://github.com/helius-labs/zolana-tvc)

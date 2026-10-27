# Embedded private wallet

A Node.js Express starter that creates and runs Solana wallets with a private balance,
with only a Helius API key. Each wallet is an embedded wallet held by Turnkey
in your Helius project, which sends SOL and SPL tokens as any Solana wallet
does. Its private keys, which read and spend the private balance, are derived
and held by the Helius enclave (Turnkey Verifiable Compute), never by your
server.

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

Run the full live devnet suite with:

```bash
cp .env.example .env  # set HELIUS_API_KEY and DEVNET_PAYER_KEYPAIR_PATH
pnpm test:devnet
```

Use a Helius API key with Wallet-as-a-Service enabled and a **devnet-only**
Solana JSON keypair with at least **0.2 SOL**. The payer path is resolved from
this directory; `.data/devnet-payer.json` is a gitignored location for it.
Never put keypair bytes or API keys in tracked files. `.env.example` contains
placeholders; `.env` and `.env.*` are ignored. Environment variables override
`.env`, so the same command can use secrets supplied by CI.

The runner verifies the devnet genesis hash before funding. It starts its own
server on a free port and creates two fresh embedded wallets per run. It checks
registration, deposits, private transfer, withdrawal, exact balances and
history, public SOL and standard SPL Token/Token-2022 sends, recipient token
account creation/reuse, rejected requests, and recovery after a server restart.
A test-only preload rejects plaintext proof traffic and records attestation
and encrypted-proof metadata without request bodies or credential headers.
Token-2022 mints in this test have no extensions.

Each run prints its directory under `.data/devnet-e2e/run-*` and exits nonzero
if any check fails. That gitignored directory contains `results.json` (check
results, public addresses and transaction signatures), `prover-audit.jsonl`
(metadata only), `server.log` (API keys redacted), and `data/wallets/` (secret
wallet credentials). The server stops on success, failure, or Ctrl-C. Run data
is retained so failed runs can be inspected and test wallets can be reopened;
keep the whole directory private.

The suite transfers **0.07 devnet SOL to each test wallet**, plus mint setup
rent and transaction fees from the payer. It leaves the new wallets, balances
and token accounts in place and does not automatically reclaim funds. This is
an opt-in live check; ordinary `pnpm check` and PR CI do not run it.

## Links

- [Documentation](https://helius.dev/docs/privacy)
- [Zolana SDK](https://github.com/helius-labs/zolana)
- [Private wallet enclave](https://github.com/helius-labs/zolana-tvc)

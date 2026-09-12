# Embedded private wallet (React)

Deposit, privately transfer, withdraw, and query SOL using a Turnkey embedded wallet and the Zolana SDK. This example uses React + Vite.

Turnkey handles authentication and Solana transaction signing. TVC (Turnkey Verifiable Compute) supplies the SDK’s private-wallet keys:

1. Sign in with Turnkey
2. Activate the private wallet and verify its onchain registration
3. Build an unsigned transaction with the Zolana SDK
4. Sign with the Turnkey wallet and submit to Solana
5. Sync private balances and transaction history

## What you will implement

| Operation | SDK / RPC | Source file |
| --- | --- | --- |
| **Register wallet** | `buildRegistrationTransaction()` | [registerWallet.ts](src/operations/registerWallet.ts) |
| **Check registration** | `fetchUserRecord()` | [registration.ts](src/lib/registration.ts) |
| **Deposit** | `buildDepositTransaction()` | [deposit.ts](src/operations/deposit.ts) |
| **Private transfer** | `buildTransferTransaction()` | [transfer.ts](src/operations/transfer.ts) |
| **Withdraw** | `buildWithdrawalTransaction()` | [withdraw.ts](src/operations/withdraw.ts) |
| **Get balance** | `connection.getBalance()`, `wallet.balance()` | [getBalance.ts](src/operations/getBalance.ts) |
| **Sync wallet** | `syncWallet()` | [syncWallet.ts](src/operations/syncWallet.ts) |
| **Get history** | `getPrivateTransactions()` | [getHistory.ts](src/operations/getHistory.ts) |
| **Sync history** | `syncWallet()` → `getPrivateTransactions()` | [syncHistory.ts](src/operations/syncHistory.ts) |

### Source files

#### Operations

Each file in [`src/operations/`](src/operations/) exposes a function that can be called from a React event handler. They share the verified [`PrivateWalletContext`](src/lib/walletContext.ts) returned by `usePrivateWallet()`.

- **[deposit.ts](src/operations/deposit.ts)** — Move public SOL into the connected wallet’s private balance.
- **[transfer.ts](src/operations/transfer.ts)** — Send private SOL to a registered recipient, reporting actual proof and transaction progress.
- **[withdraw.ts](src/operations/withdraw.ts)** — Move private SOL back to the connected wallet’s public balance.
- **[getBalance.ts](src/operations/getBalance.ts)** — Fetch public SOL or read private SOL from the last sync. Public SOL is available before activation.
- **[syncWallet.ts](src/operations/syncWallet.ts)** — Fetch indexed activity and update private balances and history using the active TVC keys.
- **[getHistory.ts](src/operations/getHistory.ts)** — Read the last synced private transaction history without a network request.
- **[syncHistory.ts](src/operations/syncHistory.ts)** — Sync first, then return private transaction history. Available as a helper; not displayed in the current UI.
- **[registerWallet.ts](src/operations/registerWallet.ts)** — Reuse a matching registration or submit and verify a new one. Conflicting identities are rejected.

Private balance and history reads use local wallet state. Sync updates both; there is no separate history-only network sync. History rows include signature, slot, kind, direction, asset, and amount. They describe indexed private-wallet activity, not every public transaction for the Solana address. A transaction can produce multiple rows.

Deposit, transfer, and withdrawal sync automatically after confirmation. If that sync fails, the confirmed explorer receipt is retained. Refresh reuses the active wallet context without repeating activation.

#### React hooks and components

- **[useEmbeddedWallet.ts](src/hooks/useEmbeddedWallet.ts)** — Turnkey login, logout, account selection, and signing.
- **[usePrivateWallet.ts](src/hooks/usePrivateWallet.ts)** — Explicit activation, TVC recovery, registration, initial sync, and session cleanup.
- **[useBootstrapApproval.ts](src/hooks/useBootstrapApproval.ts)** — Verify and approve the expected Turnkey bootstrap activity.
- **[App.tsx](src/App.tsx)** — Wallet address, public/private balances, editable amounts, and transaction controls.
- **[TransferStepper.tsx](src/TransferStepper.tsx)** — Preparing → Proving → Sending → Confirmed, driven by actual operation events.
- **[MotionRegion.tsx](src/MotionRegion.tsx)** — Smooth height changes with reduced-motion support.

## Before you start

Use **Node.js 24+**, **pnpm 11.18.0**, and a Helius project with embedded wallets enabled. This example uses **Solana devnet**.

Fund the connected Turnkey address with enough devnet SOL for the amount you want to deposit, registration rent if needed, and transaction fees. A private transfer requires sufficient **private** SOL and a recipient with a registered private wallet.

Login alone does not activate or register a private wallet. **Activate private wallet** verifies TVC, restores or creates the private identity, checks the onchain registry, and registers only when needed. Reloading with the same account restores the saved identity; registration is checked again onchain.

## Setup

From the repository root:

```bash
cd embedded-private-wallet
pnpm install
cp .env.example .env
# Fill in your Helius browser key
```

### Environment variables

| Variable | Description |
| --- | --- |
| `VITE_API_KEY` | Helius browser key for a project with embedded wallets enabled. Also supplies the default devnet RPC endpoint. |
| `VITE_ZOLANA_ENDPOINT` | Optional devnet Solana RPC override. Does not replace the Helius key required for wallet login. |

Keep `.env` local; it is ignored by Git. `VITE_` values are public browser configuration. Do not put app secrets or Turnkey operator keys in them.

The Vite dev and preview proxies route TVC and private indexer/prover requests to `https://i6npwfd4mh.eu-west-1.awsapprunner.com`. The app verifies an independently pinned trust policy. Normal Solana RPC uses the configured devnet endpoint. The static build requires an equivalent proxy when hosted; it cannot provide those routes by itself.

## Quick start

```bash
pnpm dev
```

Open [localhost:5173](http://127.0.0.1:5173/), then:

1. Sign in with Turnkey and fund the displayed devnet address.
2. Click **Activate private wallet** and approve setup when requested.
3. Choose **Deposit**, enter a SOL amount, and submit.
4. Choose **Transfer** with a registered recipient, or **Withdraw** to your connected wallet.
5. Follow the progress and open the centered **View transaction** receipt.
6. Click **Refresh balances** to sync without activating again.

Amounts support up to nine decimal places. Defaults are 0.01 SOL for deposit and 0.003 SOL for transfer and withdrawal.

To use history from a React handler after activation:

```ts
import { getPrivateHistory } from "./operations/getHistory";
import { syncPrivateHistory } from "./operations/syncHistory";

// ctx is the active context returned by usePrivateWallet().
const cachedHistory = getPrivateHistory(ctx);
const refreshedHistory = await syncPrivateHistory(ctx);
```

Guard handlers against concurrent operations and discard results when the account changes, as in `App.tsx`. Operations check the active session before returning. A sync failure throws instead of presenting cached history as a fresh result.

## Documentation

- [Privacy documentation](https://helius.dev/docs/privacy)
- [Launch demo](https://helius.dev/privacy/demo)
- [Storage and security boundary](docs/security.md) — TVC storage and the current external-prover limitation.
- [Development, tests, and validation](docs/development.md) — Build commands, motion behavior, and recorded live acceptance.
- [Pinned dependencies and rebuild instructions](vendor/README.md)

TVC keeps long-lived private keys out of the app’s normal wallet context. The current external prover receives plaintext witness material, including the nullifier secret; do not assume every secret remains exclusively inside the enclave. See the security document for details.

# Embedded private wallet (React)

Deposit, transfer public or private SOL, withdraw, and query balances using a Turnkey embedded wallet and the Zolana SDK. This example uses React + Vite.

Turnkey handles authentication and Solana transaction signing. TVC (Turnkey Verifiable Compute) supplies the SDK’s private-wallet keys:

1. Sign in with Turnkey
2. Activate the private wallet and verify its onchain registration
3. Read indexed notes and build instructions with the Zolana SDK
4. Sign with the Turnkey wallet and submit to Solana
5. Fetch and decrypt fresh private balances and transaction history

## What you will implement

| Operation | SDK / RPC | Source file |
| --- | --- | --- |
| **Register wallet** | `buildRegistrationTransaction()` | [registerWallet.ts](src/operations/send/registerWallet.ts) |
| **Check registration** | `fetchUserRecord()` | [registration.ts](src/lib/registration.ts) |
| **Deposit** | `getDepositInstructionAsync()` | [deposit.ts](src/operations/send/deposit.ts) |
| **Public transfer** | `getTransferSolInstruction()`, `getFeeForMessage()` | [publicTransfer.ts](src/operations/send/publicTransfer.ts) |
| **Private transfer** | `ConfidentialTransfer`, `getTransactInstruction()` | [transfer.ts](src/operations/send/transfer.ts) |
| **Withdraw** | `ConfidentialTransfer.withdraw()`, `getTransactInstruction()` | [withdraw.ts](src/operations/send/withdraw.ts) |
| **Get balance** | `client.getBalance()`, fresh indexed read | [getBalance.ts](src/operations/read/getBalance.ts) |
| **Get history** | `decryptTransactions()`, `wallet.privateTransactions()` | [getHistory.ts](src/operations/read/getHistory.ts) |
| **Read and decrypt** | `getShieldedTransactionsByTags()`, `getEncryptedUtxosByTags()`, `getShieldedTransactionsByNullifiers()`, `decryptTransactions()` | [readPrivateState.ts](src/lib/readPrivateState.ts) |

### Source files

#### Operations

Each file in [`src/operations/`](src/operations/) exposes a function that can be called from a React event handler. Private operations share the verified [`PrivateWalletContext`](src/lib/walletContext.ts) returned by `usePrivateWallet()`. Public transfers use a `PublicWalletContext` created from the connected Turnkey session, without TVC keys or activation.

##### Read

- **[getBalance.ts](src/operations/read/getBalance.ts)** — Fetch public SOL or reconstruct private SOL from a fresh indexed read. Public SOL is available before activation.
- **[getHistory.ts](src/operations/read/getHistory.ts)** — Fetch and decrypt confirmed private transaction history. Available as a helper; not displayed in the current UI.

##### Send

- **[deposit.ts](src/operations/send/deposit.ts)** — Move your public SOL into your private balance or another registered private wallet.
- **[publicTransfer.ts](src/operations/send/publicTransfer.ts)** — Send public SOL to a Solana address using the System Program instruction builder. Check fresh public funds against the amount plus the RPC-estimated fee before verified Turnkey signing. No private registry lookup or proof is needed.
- **[transfer.ts](src/operations/send/transfer.ts)** — Send private SOL to a registered recipient, reporting actual proof and transaction progress.
- **[withdraw.ts](src/operations/send/withdraw.ts)** — Move your private SOL to your own public balance or another Solana address.
- **[registerWallet.ts](src/operations/send/registerWallet.ts)** — Reuse a matching registration or submit and verify a new one. Conflicting identities are rejected.

Private reads are stateless. Each call creates a temporary SDK `Wallet`, reads all indexer pages, decrypts through TVC, and looks up discovered nullifiers to exclude spent notes. The temporary wallet is discarded; no scan cursors, decrypted history, or spendable-note cache survive between calls. React retains only displayed results. Turnkey login and TVC identity recovery remain persistent.

The shared [reader](src/lib/readPrivateState.ts) imports public SDK APIs; no operation imports another operation to obtain cached state. History rows include signature, slot, kind, direction, asset, and amount. They describe indexed private activity, not every public transaction for the Solana address. Multiple rows may share a signature.

Transfer and withdrawal fetch fresh spendable notes, prepare with `ConfidentialTransfer`, encrypt with a TVC transaction key, prove with `client.proveTransact`, and call `getTransactInstruction`. [compileInstructions.ts](src/lib/compileInstructions.ts) uses Solana Kit to compile the instructions before verified Turnkey signing. Registration retains `buildRegistrationTransaction` because this SDK version does not export a standalone registration instruction builder.

After private transaction confirmation, operations read again with the confirmed slot as the indexer freshness requirement. If that read fails, the explorer receipt is retained and public SOL still refreshes. Refresh reuses TVC keys without repeating activation. Full-history reads require more indexer and TVC work than incremental sync; the React balance read allows 60 seconds, independently of proof generation.

#### React hooks and components

- **[useEmbeddedWallet.ts](src/hooks/useEmbeddedWallet.ts)** — Turnkey login, logout, account selection, and signing.
- **[usePrivateWallet.ts](src/hooks/usePrivateWallet.ts)** — Explicit activation, TVC recovery, registration, initial read, and session cleanup.
- **[useBootstrapApproval.ts](src/hooks/useBootstrapApproval.ts)** — Verify and approve the expected Turnkey bootstrap activity.
- **[App.tsx](src/App.tsx)** — Wallet address, public/private balances, editable amounts, and transaction controls.
- **[TransferStepper.tsx](src/TransferStepper.tsx)** — Preparing → Proving → Sending → Confirmed for private transfers; public transfers omit Proving. Both use actual operation events.
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
2. Select **Public balance** → **Public Transfer** to send public SOL immediately. Enter the amount and recipient, then approve the transaction in Turnkey.
3. For private operations, select **Private balance** and click **Activate private wallet**. Approve setup when requested.
4. Choose **Deposit** from either balance view to fund a private wallet from your public SOL.
5. Select **Private balance** → **Private Transfer** for a registered recipient, or choose **Withdraw** from either view to send private SOL to a public wallet.
6. Follow progress and open the centered **View transaction** receipt.
7. Click **Refresh balances** to fetch current balances without activating again. When Public transfer is selected, refresh reads only public SOL.

The read-only **Total balance** adds public and private SOL without converting lamports to floating point. It shows `—` if either balance is unknown. Both balance views offer **Deposit**, **Private Transfer** or **Public Transfer**, and **Withdraw**, in that order. Deposit always spends public SOL; withdrawal always spends private SOL, regardless of the selected view.

Deposit and withdrawal default to **My wallet**. Select **Another wallet** to enter a recipient address. A deposit looks up the recipient’s registered private identity; withdrawal sends to a regular Solana address. Changing actions or balance views resets this choice to your own wallet and clears any typed recipient. The connected wallet remains the payer and receives private change.

```ts
import { address } from "@solana/kit";
import { depositSol } from "./operations/send/deposit";
import { withdrawSol } from "./operations/send/withdraw";

// ctx is the active private-wallet context; amounts are lamports.
await depositSol(ctx, 10_000_000n); // Default: your private balance.
await depositSol(ctx, 10_000_000n, address(registeredRecipient));
await withdrawSol(ctx, 3_000_000n); // Default: your public balance.
await withdrawSol(ctx, 3_000_000n, address(publicRecipient));
```

Public transfers remain available if TVC activation or private reads fail. Public and private transfers keep separate amount drafts; changing the balance selection clears the recipient. Public transfers refresh only public SOL after confirmation and retain the receipt if that refresh fails.

Amounts support up to nine decimal places. Defaults are 0.01 SOL for deposit and 0.003 SOL for transfer and withdrawal.

To use history from a React handler after activation:

```ts
import { getPrivateHistory } from "./operations/read/getHistory";

// ctx is the active context returned by usePrivateWallet().
const history = await getPrivateHistory(ctx);
```

Guard handlers against concurrent operations and discard results when the account changes, as in `App.tsx`. Operations check the active session before returning. A failed read throws instead of presenting partial history or an empty balance.

## Documentation

- [Privacy documentation](https://helius.dev/docs/privacy)
- [Launch demo](https://helius.dev/privacy/demo)
- [Storage and security boundary](docs/security.md) — TVC storage and the current external-prover limitation.
- [Development, tests, and validation](docs/development.md) — Build commands, motion behavior, and recorded live acceptance.
- [Pinned dependencies and rebuild instructions](vendor/README.md)

TVC keeps long-lived private keys out of the app’s normal wallet context. The current external prover receives plaintext witness material, including the nullifier secret; do not assume every secret remains exclusively inside the enclave. See the security document for details.

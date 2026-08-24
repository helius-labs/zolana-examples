# Sign with Wallet Adapter

Wallet Adapter connects Phantom, Solflare, and other Wallet Standard wallets. This example derives viewing and nullifier keys from one `signMessage`, then deposit, privately transfer, and withdraw on Helius devnet. The wallet does not export the Ed25519 secret.

## What stays in the wallet

```mermaid
flowchart LR
  subgraph wallet [Wallet]
    Ed25519[Ed25519 secret]
  end
  subgraph page [Example page]
    View[Viewing key]
    Nf[Nullifier key]
  end
  Ed25519 -->|"signMessage TSPP/derive/v1"| Hkdf[HKDF]
  Hkdf --> View
  Hkdf --> Nf
  Ed25519 -->|signTransaction| SolanaTx[Solana transaction]
```

The signature over `TSPP/derive/v1` is the seed for viewing and nullifier keys. Reject any third-party request to sign that message.

## User flow

```mermaid
sequenceDiagram
  participant User
  participant WalletAdapter
  participant Page
  participant Sdk
  participant Devnet

  User->>WalletAdapter: Connect
  WalletAdapter-->>Page: pubkey
  Page->>WalletAdapter: signMessage TSPP/derive/v1
  WalletAdapter-->>Page: 64-byte signature
  Page->>Page: HKDF viewing and nullifier
  Page->>Sdk: buildRegistrationTransaction
  Page->>WalletAdapter: signTransaction
  WalletAdapter-->>Page: signed tx
  Page->>Devnet: sendAndConfirm
  Page->>Sdk: syncWallet
  User->>Page: Deposit or transfer or withdraw
  Page->>Sdk: buildDepositTransaction or buildTransferTransaction or buildWithdrawalTransaction
  Page->>WalletAdapter: signTransaction
  Page->>Devnet: sendAndConfirm
  Page->>Sdk: syncWallet
```

## Deposit, private transfer, withdraw

```mermaid
flowchart TD
  Public[Public SOL] -->|deposit reveals sender recipient asset amount| Private[Private SOL]
  Private -->|private transfer reveals sender and recipient only| Recipient[Recipient private SOL]
  Private -->|withdraw reveals sender recipient asset amount| PublicOut[Public SOL]
```

## Run

Install Node.js 24+ and pnpm.

```bash
pnpm install
cp .env.example .env
# set VITE_API_KEY
pnpm dev
```

Then connect a wallet funded on devnet. The first prompt is `signMessage`. Later prompts are `signTransaction`.

## Tests

```bash
pnpm test
# live devnet; needs API_KEY and a funded ~/.config/solana/id.json
pnpm test:integration
```

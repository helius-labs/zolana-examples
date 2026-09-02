# Sign with Wallet Adapter

Wallet Adapter connects Phantom, Solflare, and other Wallet Standard wallets. This example derives viewing and nullifier keys from one `signMessage`, then deposit, privately transfer, and withdraw on Helius devnet.

## User flow

```mermaid
sequenceDiagram
  participant User
  participant Wallet
  participant Application
  participant Sdk
  participant Devnet

  User->>Wallet: Connect
  Wallet-->>Application: pubkey
  Application->>Wallet: signMessage TSPP/derive/v1
  Wallet-->>Application: 64-byte signature
  Application->>Application: HKDF viewing and nullifier
  Application->>Sdk: buildRegistrationTransaction
  Application->>Wallet: signTransaction
  Wallet-->>Application: signed tx
  Application->>Devnet: sendAndConfirm
  Application->>Sdk: syncWallet
  User->>Application: Deposit or transfer or withdraw
  Application->>Sdk: buildDepositTransaction or buildTransferTransaction or buildWithdrawalTransaction
  Application->>Wallet: signTransaction
  Application->>Devnet: sendAndConfirm
  Application->>Sdk: syncWallet
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

# Privy + Node.js Server Starter

This example showcases how to use confidential transfers, deposit and withdrawals using Privy's Node.js SDK in a server-side Express.js application.

It is based on https://github.com/privy-io/examples/tree/main/privy-node-starter and adds support for Solana Privacy Rings.

**What changes:**

- For encryption and decryption, you can extend each Privy Solana wallet with a shielded keypair.
- Its viewing and nullifier keys are derived from your Privy wallet's signature over a fixed message and held in server memory.
- Transactions use the same Privy SDK and Solana Ed25519 key for signatures.

**Important:** The derivation signature can recreate the viewing and nullifier keys. Never log or persist it or the derived private keys. Clear key buffers after use. Keeping keys in memory does not protect against a compromised server. [Key derivation and security](https://www.helius.dev/docs/privacy/integration/wallet#integrate-the-shielded-keypair).

**What is private:**

| Field     | Deposit | Transfer | Withdrawal |
| --------- | ------- | -------- | ---------- |
| Sender    | Public  | Public   | Public     |
| Recipient | Public  | Public   | Public     |
| Asset     | Public  | Private  | Public     |
| Amount    | Public  | Private  | Public     |

## Getting Started

### 1. Clone the Project

```bash
git clone https://github.com/helius-labs/zolana-examples.git
cd zolana-examples/privy-node-starter
```

### 2. Install Dependencies

Use Node.js 24 or later.

```bash
npm install
```

### 3. Configure Environment

Copy the example environment file and configure your Privy app credentials:

```bash
cp .env.example .env
```

Update `.env` with your Privy app credentials and treasury configuration:

```env
# Privy App Credentials
PRIVY_APP_ID=your_app_id_here
PRIVY_APP_SECRET=your_app_secret_here

# Treasury Configuration
TREASURY_WALLET_ID=your_treasury_wallet_id
TREASURY_WALLET_ADDRESS=your_treasury_wallet_address
TREASURY_AUTHORIZATION_KEY=your_base64_encoded_private_key
TREASURY_OWNER_ID=your_treasury_owner_id

# Key Quorum Configuration
TREASURY_QUORUM_PRIVATE_KEY=your_base64_encoded_private_key
TREASURY_QUORUM_PUBLIC_KEY=your_base64_encoded_public_key

# Policy Configuration
ALLOWLIST_USDC_POLICY_ID=your_allowlist_policy_id
```

```env
# Confidential Transactions on Devnet
HELIUS_RPC_URL=https://devnet.helius-rpc.com/?api-key=your_helius_api_key
SENDER_WALLET_ID=your_solana_sender_wallet_id

# E2E test only
RECIPIENT_WALLET_ID=your_solana_recipient_wallet_id
```

The confidential endpoints require `PRIVY_APP_ID`, `PRIVY_APP_SECRET`, `HELIUS_RPC_URL`, `SENDER_WALLET_ID`, and `TREASURY_AUTHORIZATION_KEY`. The authorization key must control the configured Solana wallet. The Ethereum treasury wallet ID is separate. Fund the Solana wallet for deposits, transaction fees, and account rent. The E2E script also requires `RECIPIENT_WALLET_ID` and a funded recipient wallet controlled by the same authorization key.

**Important:** Get your credentials from the [Privy Dashboard](https://dashboard.privy.io). Never expose `PRIVY_APP_SECRET` in client-side code.

### 4. Start Development Server

```bash
npm run dev
```

The server will start on [http://localhost:3300](http://localhost:3300).

## Core Functionality

### 1. Initialize Privy Server Client

Server-side initialization with app credentials.

[`src/lib/privy.ts`](./src/lib/privy.ts)
```typescript
import { PrivyClient } from "@privy-io/node";

export const privy = new PrivyClient({
  appId: PRIVY_APP_ID,
  appSecret: PRIVY_APP_SECRET,
});
```

### 2. Create Wallets with Key Quorums

Create wallets owned by multi-signature key quorums for enhanced security.

[`src/handlers/key_quorums/create_wallet.ts`](./src/handlers/key_quorums/create_wallet.ts)
```typescript
import { privy } from "../../lib/privy";

// Create 2-of-2 key quorum
const keyQuorum = await privy.keyQuorums().create({
  public_keys: [public_key, TREASURY_QUORUM_PUBLIC_KEY],
  authorization_threshold: 2,
  display_name: `Treasury 2-of-2 Key Quorum with ${alias}`,
});

// Create wallet owned by key quorum
const wallet = await privy.wallets().create({
  chain_type: "ethereum",
  owner_id: keyQuorum.id,
});
```

### 3. Send USDC Transactions

Execute server-side USDC transactions with policy enforcement.

[`src/handlers/transactions/send_usdc.ts`](./src/handlers/transactions/send_usdc.ts)
```typescript
import { privy } from "../../lib/privy";

// Send USDC with treasury approval
const transactionRequest = await privy.transactions().create({
  wallet_id: TREASURY_WALLET_ID,
  chain_type: "ethereum",
  // Transaction details configured server-side
});
```

### 4. Deposit into a Private Balance

Deposit SOL or SPL tokens into the configured Privy wallet's private balance.

[`src/handlers/transactions/deposit.mts`](./src/handlers/transactions/deposit.mts)

```bash
curl -X POST http://localhost:3300/transactions/deposit \
  -H "Content-Type: application/json" \
  -d '{"amount":"10000000"}'
```

Omit `mint` for SOL. To deposit SPL tokens, add `"mint":"your_token_mint"` and provide the amount in token base units. The configured wallet must hold the tokens in its associated token account.

### 5. Send Confidential Transfers

Send from the configured wallet's private balance to a registered Solana address.

[`src/handlers/transactions/transfer.mts`](./src/handlers/transactions/transfer.mts)

```bash
curl -X POST http://localhost:3300/transactions/transfer \
  -H "Content-Type: application/json" \
  -d '{"recipient":"your_registered_recipient","amount":"3000000"}'
```

The SDK resolves the recipient's public shielded keys from the registry. The recipient does not need to be a Privy wallet. Before receiving, the recipient registers its derived public shielded keys with the SDK's `getRegisterInstructionAsync` builder. Registration is setup and is not repeated for every transfer.

### 6. Withdraw from a Private Balance

Withdraw SOL or SPL tokens to a public Solana address.

[`src/handlers/transactions/withdraw.mts`](./src/handlers/transactions/withdraw.mts)

```bash
curl -X POST http://localhost:3300/transactions/withdraw \
  -H "Content-Type: application/json" \
  -d '{"recipient":"your_public_recipient","amount":"7000000"}'
```

For SPL tokens, the transaction creates the recipient's associated token account when needed.

### 7. Run the Devnet Example

Validate deposit, confidential transfer, and withdrawal for SOL and SPL tokens with two Privy Solana wallets.

[`scripts/confidential.e2e.mts`](./scripts/confidential.e2e.mts)

```bash
npm run confidential:e2e
```

Configure two distinct funded Solana wallets with the same authorized owner key. `RECIPIENT_WALLET_ID` is needed only for this script. The script registers their public shielded keys, creates a test SPL mint, starts the Express server, checks private and public balances, tests restart recovery, and withdraws both private balances. It prints confirmed transaction hashes. Port 3300 must be available.

## Relevant Links

- [Devnet Benchmarks](./BENCHMARK.md)
- [Privy Dashboard](https://dashboard.privy.io)
- [Privy Documentation](https://docs.privy.io)
- [Node.js SDK](https://www.npmjs.com/package/@privy-io/node)
- [Express.js Documentation](https://expressjs.com/)
- [Helius Privacy Documentation](https://www.helius.dev/docs/privacy)
- [Deposit into a Private Balance](https://www.helius.dev/docs/privacy/guides/deposit)
- [Private Transfers](https://www.helius.dev/docs/privacy/guides/transfer)
- [Withdraw from a Private Balance](https://www.helius.dev/docs/privacy/guides/withdraw)
- [Helius Privacy Demo](https://helius.dev/privacy/demo)

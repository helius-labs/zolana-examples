import {
  ShieldedKeypair,
  Wallet,
  createZolanaClient,
  syncWallet,
} from "@heliuslabs/zolana";
import {
  AssetRegistry,
  LocalShieldedKeys,
} from "@heliuslabs/zolana/transaction";

import { cliKeypair } from "../src/lib.js";

const client = await createZolanaClient({
  solanaRpcUrl: `https://devnet.helius-rpc.com/?api-key=${process.env.API_KEY}`,
});
// localnet: zolana dev start. RPC port :8899, indexer port :8784, prover port :3001.
// const client = await createZolanaClient({});

// Initialize the sender's private wallet and local authority
// to decrypt transactions and sync balances.
// The Solana signer and private wallet are derived from the same Ed25519 seed.
const sender = ShieldedKeypair.fromKeypair(
  await cliKeypair(),
);
const assets = new AssetRegistry();

// Sync all transaction pages and resolve SPL asset registrations.
const wallet = new Wallet({
  identity: sender.shieldedAddress(),
  registry: assets,
});
await syncWallet({
  wallet,
  keys: LocalShieldedKeys.fromKeypair(sender),
  client,
  config: { pageLimit: 50 },
});

for (const tx of wallet.privateTransactions()) {
  console.log(
    `ok kind=${tx.kind} direction=${tx.direction} mint=${tx.asset} amount=${tx.amount} tx=${tx.id.signature}`,
  );
}

import {
  ShieldedKeypair,
  Wallet,
  createZolanaClient,
} from "@heliuslabs/zolana";
import {
  AssetRegistry,
  LocalShieldedKeys,
  decryptTransactions,
} from "@heliuslabs/zolana/transaction";

import { cliKeypair, setup } from "../src/lib.js";

async function main(): Promise<void> {
  const { clientConfig } = await setup();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const sender = ShieldedKeypair.fromKeypair(
    await cliKeypair(),
  );
  const assets = new AssetRegistry();

  // Fetch transaction outputs from the indexer.
  const response =
    await client.getShieldedTransactionsByTags({
      tags: [
        sender
          .shieldedAddress()
          .confidentialViewTag(),
      ],
    });

  // Decrypt locally to read the private history.
  const wallet = new Wallet({
    identity: sender.shieldedAddress(),
    registry: assets,
  });
  await decryptTransactions({
    wallet,
    keys: LocalShieldedKeys.fromKeypair(sender),
    transactions: response.transactions,
  });

  for (const tx of wallet.privateTransactions()) {
    console.log(
      `ok kind=${tx.kind} direction=${tx.direction} mint=${tx.asset} amount=${tx.amount} tx=${tx.id.signature}`,
    );
  }
}

await main();

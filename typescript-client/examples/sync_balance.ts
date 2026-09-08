import {
  ShieldedKeypair,
  createZolanaClient,
} from "@heliuslabs/zolana";
import {
  AssetRegistry,
  decryptToBalances,
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

  // Decrypt locally to read the private balances.
  const balances = await decryptToBalances({
    keypair: sender,
    registry: assets,
    transactions: response.transactions,
  });

  const solanaAddress = sender
    .shieldedAddress()
    .solanaAddress();
  for (const b of balances.balances()) {
    console.log(
      `ok solana_address=${solanaAddress} mint=${b.mint} amount=${b.amount} utxos=${b.utxos.length}`,
    );
  }
}

await main();

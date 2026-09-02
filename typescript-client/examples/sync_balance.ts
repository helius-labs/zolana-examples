import { createZolanaClient } from "@heliuslabs/zolana";
import {
  AssetRegistry,
  decryptToBalances,
} from "@heliuslabs/zolana/transaction";

import { connect } from "../src/lib.js";

async function main(): Promise<void> {
  const { wallet, clientConfig } =
    await connect();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  const assets = new AssetRegistry();
  const address = wallet.shieldedAddress();

  // Fetch transaction outputs from the indexer.
  const response =
    await client.getShieldedTransactionsByTags({
      tags: [address.confidentialViewTag()],
    });

  // Decrypt locally to read the private balances.
  const balances = await decryptToBalances({
    keypair: wallet,
    registry: assets,
    transactions: response.transactions,
  });

  const solanaAddress = address.solanaAddress();
  for (const b of balances.balances()) {
    console.log(
      `ok solana_address=${solanaAddress} mint=${b.mint} amount=${b.amount} utxos=${b.utxos.length}`,
    );
  }
}

await main();

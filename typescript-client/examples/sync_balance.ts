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

  const solanaAddress = sender
    .shieldedAddress()
    .solanaAddress();
  for (const b of wallet.balances()) {
    console.log(
      `ok solana_address=${solanaAddress} mint=${b.mint} amount=${b.amount} utxos=${b.utxos.length}`,
    );
  }
}

await main();

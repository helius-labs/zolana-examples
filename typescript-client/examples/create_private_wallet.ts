import {
  ShieldedKeypair,
  buildRegistrationTransaction,
  createZolanaClient,
} from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";

import {
  cliKeypair,
  sendTransactionFactory,
  setup,
} from "../src/lib.js";

async function main(): Promise<void> {
  const { clientConfig } = await setup();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  const sender = ShieldedKeypair.fromKeypair(
    await cliKeypair(),
  );
  const senderSigner = sender.toSolanaSigner();

  // The SDK hands back a transaction; the app owns signing and sending.
  const sendTransaction = sendTransactionFactory(
    client,
    senderSigner,
  );

  // Create a private wallet. This registers inbox -> shielded_public_key in the protocol registry.
  const registration =
    await buildRegistrationTransaction({
      client,
      owner: senderSigner.address,
      address: sender.shieldedAddress(),
    });
  if (registration !== undefined) {
    await sendTransaction(registration);
  }

  const registered = await isWalletRegistered({
    rpc: client,
    owner: senderSigner.address,
  });
  if (!registered) {
    throw new Error(
      "expected the wallet to be registered",
    );
  }

  console.log(
    `ok private wallet solana_address=${senderSigner.address}`,
  );
}

await main();

import {
  buildRegistrationTransaction,
  createZolanaClient,
} from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";

import {
  sendTransactionFactory,
  setup,
} from "../src/lib.js";

async function main(): Promise<void> {
  const {
    sender: senderKeypair,
    clientConfig,
  } = await setup();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const senderSigner =
    senderKeypair.toSolanaSigner();
  const senderAddress =
    senderKeypair.shieldedAddress();

  // The SDK hands back a transaction; the app owns signing and sending.
  const sendTransaction = sendTransactionFactory(
    client,
    senderSigner,
  );

  // Create a private wallet. This adds the wallet address to a lookup table for private transfers.
  const registration =
    await buildRegistrationTransaction({
      client,
      owner: senderSigner.address,
      address: senderAddress,
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

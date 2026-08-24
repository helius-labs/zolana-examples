import {
  buildRegistrationTransaction,
  createZolanaClient,
} from "@heliuslabs/zolana";

import {
  sendCompiledFactory,
  setup,
} from "../src/lib.js";

async function main(): Promise<void> {
  const { sender, clientConfig } = await setup();

  // Connect to Helius devnet RPC plus the Photon indexer and prover.
  const client =
    await createZolanaClient(clientConfig);

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const senderSigner = sender.toSolanaSigner();
  const senderAddress = sender.shieldedAddress();

  // The SDK hands back a transaction; the app owns signing and sending.
  const sendAndConfirm = sendCompiledFactory(
    client,
    senderSigner,
  );

  // Create a private wallet. This registers the Solana address
  // so others can send private transfers to it.
  const registration =
    await buildRegistrationTransaction({
      client,
      owner: senderSigner.address,
      address: senderAddress,
    });
  if (registration === undefined) {
    throw new Error(
      "expected a registration transaction",
    );
  }
  const registrationTx =
    await sendAndConfirm(registration);

  console.log(
    `ok private wallet solana_address=${senderSigner.address} ` +
      `tx=${registrationTx.signature}`,
  );
}

await main();

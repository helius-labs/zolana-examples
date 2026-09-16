import {
  ShieldedKeypair,
  SigningKey,
  buildRegistrationTransaction,
  createZolanaClient,
} from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";

import {
  cliKeypair,
  sendAndConfirmFactory,
  sendTransactionFactory,
  setup,
  transferLamportsInstruction,
} from "../src/lib.js";

const FUND_LAMPORTS = 10_000_000n;

async function main(): Promise<void> {
  const { clientConfig } = await setup();

  // Connect to the RPC, indexer, and prover.
  const client =
    await createZolanaClient(clientConfig);

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const sender = ShieldedKeypair.fromKeypair(
    SigningKey.generate("ed25519"),
  );
  const senderSigner = sender.toSolanaSigner();

  // The SDK hands back a transaction; the CLI functions as sponsor to sign and send.
  const payer = ShieldedKeypair.fromKeypair(
    await cliKeypair(),
  ).toSolanaSigner();
  await sendAndConfirmFactory(
    client,
    payer,
  )([
    transferLamportsInstruction(
      payer.address,
      senderSigner.address,
      FUND_LAMPORTS,
    ),
  ]);
  const registration =
    await buildRegistrationTransaction({
      client,
      owner: senderSigner.address,
      address: sender.shieldedAddress(),
    });
  if (registration !== undefined) {
    await sendTransactionFactory(
      client,
      senderSigner,
    )(registration);
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

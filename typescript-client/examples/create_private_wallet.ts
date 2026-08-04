import {
  Wallet,
  buildRegistrationTransaction,
} from "@zolana/sdk";
import { isWalletRegistered } from "@zolana/sdk/wallet";

import {
  exampleContext,
  sendAndConfirmTransaction,
} from "../src/lib.js";

async function main(): Promise<void> {
  // Load the funded fee payer and network settings, then connect.
  const { client, signer, keypair } =
    await exampleContext();

  // A private wallet is an in-memory object; there is nothing on-chain to
  // create for it.
  const wallet = new Wallet({
    identity: keypair.shieldedAddress(),
  });

  // Publish the shielded keys to the on-chain registry so senders can route a
  // private transfer by Solana address. Idempotent: it does nothing if the
  // record is already current.
  const registeredBefore =
    await isWalletRegistered({
      rpc: client,
      owner: signer.address,
    });
  const transaction =
    await buildRegistrationTransaction({
      client,
      owner: signer.address,
      address: keypair.shieldedAddress(),
    });
  const signature = transaction
    ? await sendAndConfirmTransaction(
        client,
        signer,
        transaction,
      )
    : undefined;

  console.log(
    `ok private wallet solana_address=${signer.address} shielded_address=${wallet.identity.toString()} registered_before=${String(
      registeredBefore,
    )} registration_tx=${signature ?? "current"}`,
  );
}

await main();

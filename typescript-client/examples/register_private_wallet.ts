import {
  ShieldedKeypair,
  SigningKey,
  createZolanaClient,
} from "@heliuslabs/zolana";
import { getUserRecordPda } from "@heliuslabs/zolana/addresses";
import {
  getRegisterInstruction,
  getSetMergingEnabledInstruction,
} from "@heliuslabs/zolana/instructions";
import { fetchUserRecord } from "@heliuslabs/zolana/wallet";

import {
  cliKeypair,
  sendAndConfirmFactory,
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
  const owner = senderSigner.address;
  const shieldedAddress =
    sender.shieldedAddress();

  // The CLI wallet funds the sender, who pays for its own registration.
  const payer = ShieldedKeypair.fromKeypair(
    await cliKeypair(),
  ).toSolanaSigner();
  await sendAndConfirmFactory(
    client,
    payer,
  )([
    transferLamportsInstruction(
      payer.address,
      owner,
      FUND_LAMPORTS,
    ),
  ]);

  // Register the private wallet and enable merging in one transaction.
  // The registry record lives under the Solana address and publishes the
  // private wallet's keys, so others can send to it by Solana address.

  // 1. Derive the record address the registry program creates for this owner.
  const { address: userRecord } =
    await getUserRecordPda(owner);

  // 2. Build the registration instruction with the wallet's public keys.
  // `register` creates the record with merging disabled, so it must come first.
  const registerIx = getRegisterInstruction({
    userRecord,
    owner: senderSigner,
    nullifierPublicKey:
      shieldedAddress.nullifierPublicKey,
    viewingPublicKey:
      shieldedAddress.viewingPublicKey.toBytes(),
  });

  // 3. Opt the wallet into `merge_transact`, which lets a merge service
  // consolidate its private balance into fewer UTXOs.
  const setMergingEnabledIx =
    getSetMergingEnabledInstruction({
      userRecord,
      owner: senderSigner,
      enabled: true,
    });

  // 4. Send and confirm like any Solana transaction.
  const registrationTx =
    await sendAndConfirmFactory(
      client,
      senderSigner,
    )([registerIx, setMergingEnabledIx]);

  // 5. Read the record back from the registry.
  const record = await fetchUserRecord({
    rpc: client,
    owner,
  });
  if (record === undefined) {
    throw new Error(
      `expected a user record for ${owner}`,
    );
  }
  if (!record.mergingEnabled) {
    throw new Error(
      "expected merging_enabled=true after registration",
    );
  }

  console.log(
    `ok private wallet solana_address=${owner} ` +
      `user_record=${userRecord} ` +
      `merging_enabled=${record.mergingEnabled} ` +
      `tx=${registrationTx.signature}`,
  );
  console.log(
    `https://explorer.solana.com/tx/${registrationTx.signature}?cluster=devnet`,
  );
}

await main();

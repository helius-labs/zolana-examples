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

  // Derive your Solana keypair for signing and the shielded keypair for
  // encryption from the same Ed25519 seed.
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

  // Register the Solana address in the onchain registry so it can receive
  // private transfers. Anyone can look up the record to check this.

  // 1. Derive the registry record PDA from the owner's Solana address and
  // build the registration instruction.
  const { address: userRecord } =
    await getUserRecordPda(owner);
  const registerIx = getRegisterInstruction({
    userRecord,
    owner: senderSigner,
    nullifierPublicKey:
      shieldedAddress.nullifierPublicKey,
    viewingPublicKey:
      shieldedAddress.viewingPublicKey.toBytes(),
  });

  // 2. Add `merging_enabled = true` in the same transaction,
  // so the SDK can merge fragmented UTXOs in the background.
  const setMergingEnabledIx =
    getSetMergingEnabledInstruction({
      userRecord,
      owner: senderSigner,
      enabled: true,
    });

  // 3. Send and confirm like any Solana transaction.
  const registrationTx =
    await sendAndConfirmFactory(
      client,
      senderSigner,
    )([registerIx, setMergingEnabledIx]);

  // 4. Read the record back from the registry.
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

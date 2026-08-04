import {
  SOL_MINT,
  buildTransferTransaction,
  syncWallet,
} from "@zolana/sdk";

import {
  recipientAddress,
  sendAndConfirmTransaction,
  setupFundedWallet,
  walletAuthorityFromSync,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;
const TRANSFER_AMOUNT = 300_000_000n;

async function main(): Promise<void> {
  // Setup: connect, register the sender, and fund its private SOL balance.
  const { client, signer, wallet, authority } =
    await setupFundedWallet(DEPOSIT_AMOUNT);
  const recipient = recipientAddress();

  // Confidential SOL transfer to the recipient's private balance.
  // A confidential transfer reveals only sender and recipient, not the asset
  // or amount.

  // 1. Build, sign, prove, and submit the transfer. The action resolves the
  // recipient's private wallet from its Solana address.
  const transaction =
    await buildTransferTransaction({
      client,
      wallet,
      authority:
        walletAuthorityFromSync(authority),
      feePayer: signer.address,
      recipient,
      amount: TRANSFER_AMOUNT,
    });
  const signature =
    await sendAndConfirmTransaction(
      client,
      signer,
      transaction,
    );
  await client.confirmPrivateTransaction(
    signature,
  );

  // 2. Sync the sender's wallet to decrypt the private change.
  await syncWallet({
    client,
    wallet,
    authority,
    config: { waitForIndexer: true },
  });

  // 3. Read the remaining private balance.
  console.log(
    `ok private transfer signature=${signature} remaining_private_sol=${wallet.balance(SOL_MINT).amount}`,
  );
}

await main();

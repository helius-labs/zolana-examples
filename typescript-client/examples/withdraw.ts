import {
  SOL_MINT,
  buildWithdrawalTransaction,
  syncWallet,
} from "@zolana/sdk";

import {
  recipientAddress,
  sendAndConfirmTransaction,
  setupFundedWallet,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;
const WITHDRAW_AMOUNT = 300_000_000n;

async function main(): Promise<void> {
  // Setup: connect, register the sender, and fund its private SOL balance.
  const { client, signer, wallet, authority } =
    await setupFundedWallet(DEPOSIT_AMOUNT);
  const recipient = recipientAddress();

  // Withdraw SOL from the sender's private balance to a public balance.
  // A withdrawal reveals sender, recipient, asset, and amount.

  // 1. Build, sign, prove, and submit the private-to-public withdrawal. The
  // recipient can be the owner or any third party.
  const transaction =
    await buildWithdrawalTransaction({
      client,
      wallet,
      authority,
      feePayer: signer.address,
      recipient,
      amount: WITHDRAW_AMOUNT,
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

  // 2. Sync the sender's wallet to decrypt the remaining private change.
  await syncWallet({
    client,
    wallet,
    authority,
  });

  // 3. Report the public recipient and remaining private balance.
  console.log(
    `ok withdrawal signature=${signature} recipient=${recipient} remaining_private_sol=${wallet.balance(SOL_MINT).amount}`,
  );
}

await main();

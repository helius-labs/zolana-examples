import {
  SOL_MINT,
  syncWallet,
  withdraw,
} from "@zolana/sdk";

import {
  recipientAddress,
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
  const submitted = await withdraw({
    client,
    wallet,
    authority,
    feePayer: signer,
    recipient,
    amount: WITHDRAW_AMOUNT,
  });

  // 2. Sync the sender's wallet to decrypt the remaining private change.
  await syncWallet({
    client,
    wallet,
    authority,
    config: { waitForIndexer: true },
  });

  // 3. Report the public recipient and remaining private balance.
  console.log(
    `ok withdrawal signature=${submitted.signature} recipient=${recipient} remaining_private_sol=${wallet.balance(SOL_MINT).amount}`,
  );
}

await main();

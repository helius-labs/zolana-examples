import {
  getPrivateTokenBalances,
  syncWallet,
} from "@zolana/sdk";

import { setupFundedWallet } from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;

async function main(): Promise<void> {
  // Setup: connect, register the wallet, and deposit SOL into it.
  const { client, keypair, wallet, authority } =
    await setupFundedWallet(DEPOSIT_AMOUNT);

  // Sync the wallet, then read the private balance per asset.
  await syncWallet({
    client,
    wallet,
    authority,
    config: { waitForIndexer: true },
  });
  const balances =
    getPrivateTokenBalances(wallet);

  // The raw layer beneath sync: query the indexer for encrypted outputs
  // matching the wallet's public viewing key. Sync scans the wallet's complete
  // tag set and decrypts matching transactions.
  const viewTag = keypair
    .shieldedAddress()
    .confidentialViewTag();
  const response =
    await client.getShieldedTransactionsByTags(
      viewTag,
    );

  console.log(
    `ok private_balances=${String(balances.length)} encrypted_transactions=${String(
      response.transactions.length,
    )}`,
  );
}

await main();

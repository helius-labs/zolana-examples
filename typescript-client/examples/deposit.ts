import {
  LocalWalletAuthority,
  SOL_MINT,
  Wallet,
  buildDepositTransaction,
  getPrivateTokenBalances,
  syncWallet,
} from "@zolana/sdk";

import {
  exampleContext,
  sendAndConfirmTransaction,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;

async function main(): Promise<void> {
  // Load the funded fee payer and network settings, then connect.
  const { client, signer, keypair } =
    await exampleContext();

  // Initialize the sender's private wallet and local authority to decrypt
  // transactions and sync balances. The Solana signer and private wallet are
  // derived from the same Ed25519 seed.
  const wallet = new Wallet({
    identity: keypair.shieldedAddress(),
  });
  const authority = new LocalWalletAuthority({
    solanaPublicKey: signer.address,
    keypair,
  });

  // Deposit SOL into the sender's private balance.
  // A deposit from a public balance reveals sender, recipient, asset, and
  // amount. Alternatively, you can onramp fiat directly to a private balance.

  // 1. Build, sign, and send the public-to-private deposit.
  const transaction =
    await buildDepositTransaction({
      client,
      feePayer: signer.address,
      recipient: keypair.shieldedAddress(),
      amount: DEPOSIT_AMOUNT,
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

  // 2. Fetch and decrypt the sender's matching outputs from the indexer.
  await syncWallet({
    client,
    wallet,
    authority,
  });

  // 3. Read the private balance per asset.
  const balances =
    getPrivateTokenBalances(wallet);
  console.log(
    `ok deposit signature=${signature} private_sol=${wallet.balance(SOL_MINT).amount} balances=${String(
      balances.length,
    )}`,
  );
}

await main();

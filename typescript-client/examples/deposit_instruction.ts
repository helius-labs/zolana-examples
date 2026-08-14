import { SOL_MINT } from "@zolana/sdk";
import { atSlot } from "@zolana/sdk/client";
import {
  depositInstruction,
  DepositAsset,
} from "@zolana/sdk/interface";
import { randomBlinding } from "@zolana/sdk/keypair";
import {
  AssetRegistry,
  decryptToBalances,
} from "@zolana/sdk/transaction";

import {
  exampleContext,
  sendAndConfirmFactory,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;

async function main(): Promise<void> {
  const {
    client,
    signer: senderSigner,
    keypair: senderKeypair,
  } = await exampleContext();
  const sendAndConfirm = sendAndConfirmFactory(
    client,
    senderSigner,
  );
  const assets = new AssetRegistry();

  // Initialize the sender's private wallet and local authority
  // to decrypt transactions and sync balances.
  // The Solana signer and private wallet are derived from the same Ed25519 seed.
  const senderAddress =
    senderKeypair.shieldedAddress();

  // Deposit SOL into the sender's private balance.
  // A deposit from a public balance reveals
  // sender, recipient, asset and amount.
  // Alternatively, you can onramp fiat directly to a private balance.

  // 1. Move public SOL into the sender's private balance.
  // The view tag is the sender's Solana public key in confidential rings.
  // Used by the indexer to fetch the sender's outputs.
  const senderViewTag =
    senderAddress.confidentialViewTag();
  const depositIx = await depositInstruction({
    tree: client.tree,
    depositor: senderSigner,
    deposits: [
      {
        asset: DepositAsset.sol(),
        viewTag: senderViewTag,
        recipientOwnerHash:
          senderAddress.ownerHash(),
        blinding: randomBlinding(),
        amount: DEPOSIT_AMOUNT,
      },
    ],
  });

  // 2. Send and confirm like any Solana transaction; confirmation yields the landed slot.
  const depositTx = await sendAndConfirm([
    depositIx,
  ]);

  // 3. Fetch transaction outputs from the indexer, gated on the deposit's slot.
  // The indexer returns encrypted outputs by view tag.
  const depositResponse =
    await client.getShieldedTransactionsByTags(
      { tags: [senderViewTag] },
      atSlot(depositTx.slot),
    );

  // 4. The sender decrypts the transaction outputs locally to read the private balance.
  const balancesAfterDeposit =
    await decryptToBalances({
      keypair: senderKeypair,
      registry: assets,
      transactions: depositResponse.transactions,
    });
  const depositBalance =
    balancesAfterDeposit.balance(SOL_MINT);
  if (depositBalance.amount !== DEPOSIT_AMOUNT) {
    throw new Error(
      `expected deposit amount ${DEPOSIT_AMOUNT}, got ${depositBalance.amount}`,
    );
  }
  if (depositBalance.utxos.length !== 1) {
    throw new Error(
      `expected 1 deposit utxo, got ${depositBalance.utxos.length}`,
    );
  }

  console.log(
    `ok deposit private_balance=${depositBalance.amount} tx=${depositTx.signature}`,
  );
}

await main();

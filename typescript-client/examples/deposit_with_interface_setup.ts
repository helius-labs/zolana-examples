import {
  ShieldedKeypair,
  createZolanaClient,
} from "@heliuslabs/zolana";
import {
  getSplAssetRegistryAddress,
  getSplAssetVaultAddress,
} from "@heliuslabs/zolana/addresses";
import { atSlot } from "@heliuslabs/zolana/client";
import { getCreateSplInterfaceInstructionAsync } from "@heliuslabs/zolana/instructions";
import {
  decodeSplAssetRegistry,
  depositInstruction,
  DepositAsset,
} from "@heliuslabs/zolana/interface";
import {
  AssetRegistry,
  decryptToBalances,
} from "@heliuslabs/zolana/transaction";
import { TOKEN_PROGRAM_ADDRESS } from "@solana-program/token";
import type { Instruction } from "@solana/kit";

import {
  cliKeypair,
  sendAndConfirmFactory,
  setup,
  setupTestToken,
} from "../src/lib.js";

const DEPOSIT_AMOUNT = 1_000_000_000n;

async function main(): Promise<void> {
  const { clientConfig } = await setup();
  const senderKeypair =
    ShieldedKeypair.fromKeypair(
      await cliKeypair(),
    );
  const client =
    await createZolanaClient(clientConfig);
  const senderSigner =
    senderKeypair.toSolanaSigner();
  const senderAddress =
    senderKeypair.shieldedAddress();
  const sendAndConfirm = sendAndConfirmFactory(
    client,
    senderSigner,
  );

  const { mint, sourceToken } =
    await setupTestToken(
      client,
      senderSigner,
      DEPOSIT_AMOUNT,
    );

  // 1. Check whether the mint already has an interface PDA, the escrow that holds deposited tokens.
  // A second create fails the transaction, so skip step 2 when the account exists.
  const interfaceAddress =
    await getSplAssetVaultAddress(mint);
  const interfaceAccount =
    await client.getAccount(interfaceAddress);
  const instructions: Instruction[] = [];

  // 2. Create the mint registry PDA and interface token account.
  // The authority signer pays their rent.
  if (!interfaceAccount) {
    const createInterfaceIx =
      await getCreateSplInterfaceInstructionAsync(
        {
          authority: senderSigner,
          mint,
          tokenProgram: TOKEN_PROGRAM_ADDRESS,
        },
      );
    instructions.push(createInterfaceIx);
  }

  // 3. Move public tokens into the sender's private balance.
  // A deposit from a public balance reveals sender, recipient, asset and amount.
  const depositIx = await depositInstruction({
    tree: client.tree,
    depositor: senderSigner,
    deposits: [
      {
        asset: DepositAsset.spl({
          mint,
          sourceTokenAccount: sourceToken,
          tokenProgram: TOKEN_PROGRAM_ADDRESS,
        }),
        viewTag:
          senderAddress.confidentialViewTag(),
        recipientOwnerHash:
          senderAddress.ownerHash(),
        amount: DEPOSIT_AMOUNT,
      },
    ],
  });

  // 4. Send the instructions in one transaction.
  instructions.push(depositIx);
  const depositTx =
    await sendAndConfirm(instructions);

  // 5. Register the assigned asset ID for the SDK's balance lookup.
  const registryAddress =
    await getSplAssetRegistryAddress(mint);
  const registryAccount = await client.getAccount(
    registryAddress,
  );
  if (!registryAccount) {
    throw new Error(
      "mint registry missing after interface setup",
    );
  }
  const registry = decodeSplAssetRegistry(
    registryAccount.data,
  );
  const assets = new AssetRegistry();
  assets.insert(registry.assetId, registry.mint);

  // 6. Fetch this transaction's outputs, gated on its confirmed slot.
  const depositResponse =
    await client.getShieldedTransactionsBySignature(
      depositTx.signature,
      atSlot(depositTx.slot),
    );

  // 7. The sender decrypts the transaction outputs locally to read the private balance.
  const transactions =
    depositResponse.transactions.map(
      ({ transaction }) => transaction,
    );
  const balances = await decryptToBalances({
    keypair: senderKeypair,
    registry: assets,
    transactions,
  });
  const depositBalance = balances.balance(mint);
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
    `deposit mint=${mint} private_balance=${depositBalance.amount} ` +
      `tx=${depositTx.signature}`,
  );
}

await main();

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
import { getCreateAccountInstruction } from "@solana-program/system";
import {
  getInitializeAccount3Instruction,
  getInitializeMint2Instruction,
  getMintSize,
  getMintToInstruction,
  getTokenSize,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { generateKeyPairSigner } from "@solana/kit";

import {
  cliKeypair,
  sendAndConfirmFactory,
  setup,
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

  // Prepare a test mint and public tokens before the interface setup and deposit.
  const mint = await generateKeyPairSigner();
  const sourceToken =
    await generateKeyPairSigner();
  const mintRent = await client.solanaRpc
    .getMinimumBalanceForRentExemption(
      BigInt(getMintSize()),
    )
    .send();
  const tokenRent = await client.solanaRpc
    .getMinimumBalanceForRentExemption(
      BigInt(getTokenSize()),
    )
    .send();
  await sendAndConfirm([
    getCreateAccountInstruction({
      payer: senderSigner,
      newAccount: mint,
      lamports: mintRent,
      space: getMintSize(),
      programAddress: TOKEN_PROGRAM_ADDRESS,
    }),
    getInitializeMint2Instruction({
      mint: mint.address,
      decimals: 9,
      mintAuthority: senderSigner.address,
      freezeAuthority: null,
    }),
    getCreateAccountInstruction({
      payer: senderSigner,
      newAccount: sourceToken,
      lamports: tokenRent,
      space: getTokenSize(),
      programAddress: TOKEN_PROGRAM_ADDRESS,
    }),
    getInitializeAccount3Instruction({
      account: sourceToken.address,
      mint: mint.address,
      owner: senderSigner.address,
    }),
    getMintToInstruction({
      mint: mint.address,
      token: sourceToken.address,
      mintAuthority: senderSigner,
      amount: DEPOSIT_AMOUNT,
    }),
  ]);

  // 1. Fetch the interface PDA. A fresh mint has no interface yet.
  const vault = await getSplAssetVaultAddress(
    mint.address,
  );
  if (await client.getAccount(vault)) {
    throw new Error(
      "expected the test mint's interface PDA to be absent",
    );
  }

  // 2. Create the mint registry PDA and token vault. The sender pays their rent.
  const createInterfaceIx =
    await getCreateSplInterfaceInstructionAsync({
      authority: senderSigner,
      mint: mint.address,
      tokenProgram: TOKEN_PROGRAM_ADDRESS,
    });

  // 3. Move public tokens into the sender's private balance.
  // A deposit from a public balance reveals sender, recipient, asset and amount.
  const senderViewTag =
    senderAddress.confidentialViewTag();
  const depositIx = await depositInstruction({
    tree: client.tree,
    depositor: senderSigner,
    deposits: [
      {
        asset: DepositAsset.spl({
          mint: mint.address,
          sourceTokenAccount: sourceToken.address,
          tokenProgram: TOKEN_PROGRAM_ADDRESS,
        }),
        viewTag: senderViewTag,
        recipientOwnerHash:
          senderAddress.ownerHash(),
        amount: DEPOSIT_AMOUNT,
      },
    ],
  });

  // 4. Send both instructions in one transaction; confirmation yields the landed slot.
  const depositTx = await sendAndConfirm([
    createInterfaceIx,
    depositIx,
  ]);

  // 5. Register the assigned asset ID for the SDK's balance lookup.
  const registryAddress =
    await getSplAssetRegistryAddress(
      mint.address,
    );
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
  const balancesAfterDeposit =
    await decryptToBalances({
      keypair: senderKeypair,
      registry: assets,
      transactions:
        depositResponse.transactions.map(
          ({ transaction }) => transaction,
        ),
    });
  const depositBalance =
    balancesAfterDeposit.balance(mint.address);
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
    `deposit mint=${mint.address} private_balance=${depositBalance.amount} ` +
      `tx=${depositTx.signature}`,
  );
}

await main();

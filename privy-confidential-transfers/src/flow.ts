import { SOL_MINT, Wallet, createZolanaClient, initializePoseidon } from "@heliuslabs/zolana";
import { atSlot, type LocalKeys } from "@heliuslabs/zolana/client";
import {
  DepositAsset,
  TransactWithdrawal,
  depositInstruction,
  transactInstruction,
} from "@heliuslabs/zolana/interface";
import { getRegisterInstructionAsync } from "@heliuslabs/zolana/instructions";
import { fetchUserRecord } from "@heliuslabs/zolana/wallet";
import {
  ConfidentialTransfer,
  ProofInputUtxo,
  WithdrawalTarget,
  decryptTransactions,
  encryptConfidentialTransfer,
} from "@heliuslabs/zolana/transaction";
import { getSetComputeUnitLimitInstruction } from "@solana-program/compute-budget";
import {
  appendTransactionMessageInstructions,
  assertIsTransactionWithBlockhashLifetime,
  createTransactionMessage,
  getSignatureFromTransaction,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Instruction,
  type TransactionPartialSigner,
} from "@solana/kit";
import { activate, equalBytes, remoteSigner, type RemoteWallet } from "./signer.js";

export type Client = Awaited<ReturnType<typeof createZolanaClient>>;
export const DEPOSIT_AMOUNT = 10_000_000n;
export const TRANSFER_AMOUNT = 3_000_000n;

export async function connect(config: Parameters<typeof createZolanaClient>[0]) {
  await initializePoseidon();
  const client = await createZolanaClient(config);
  const genesisHash = await client.solanaRpc.getGenesisHash().send();
  if (genesisHash !== "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG") {
    throw new Error("This validation requires Solana Devnet");
  }
  return client;
}

// Reference: embedded-private-wallet/src/lib/compileInstructions.ts and typescript-client/src/lib.ts.
export async function send(
  client: Client,
  payer: TransactionPartialSigner,
  instructions: readonly Instruction[],
) {
  const { value: lifetime } = await client.solanaRpc.getLatestBlockhash().send();
  const signed = await signTransactionMessageWithSigners(
    pipe(
      createTransactionMessage({ version: 0 }),
      (message) => setTransactionMessageFeePayerSigner(payer, message),
      (message) => setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
      (message) =>
        appendTransactionMessageInstructions(
          [getSetComputeUnitLimitInstruction({ units: 450_000 }), ...instructions],
          message,
        ),
    ),
  );
  assertIsTransactionWithBlockhashLifetime(signed);
  await sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc })(signed, {
    commitment: "confirmed",
  });
  const signature = getSignatureFromTransaction(signed);
  const slot = await client.confirmTransaction(signature);
  return { signature, slot };
}

export async function validateTransfers(
  client: Client,
  senderWallet: RemoteWallet,
  recipientWallet: RemoteWallet,
  report: (stage: string, details?: unknown) => void = () => {},
) {
  if (senderWallet.address === recipientWallet.address)
    throw new Error("Select two different Privy wallets");
  const senderSigner = remoteSigner(senderWallet);
  const recipientSigner = remoteSigner(recipientWallet);
  const senderKeys = await activate(senderWallet, client.proofService);
  const recipientKeys = await activate(recipientWallet, client.proofService);
  const restoredSender = await activate(senderWallet, client.proofService);
  try {
    if (!equalBytes(senderKeys.address().toBytes(), restoredSender.address().toBytes())) {
      throw new Error("Privy message signing did not recover the same private wallet");
    }
    report("activation and wallet recovery passed");
    const sender = new Wallet({ identity: senderKeys.address() });
    const recipient = new Wallet({ identity: recipientKeys.address() });
    const transactions: Record<string, { signature: string; slot: string }> = {};

    for (const [name, signer, keys] of [
      ["sender", senderSigner, senderKeys],
      ["recipient", recipientSigner, recipientKeys],
    ] as const) {
      const existing = await fetchUserRecord({ rpc: client, owner: signer.address });
      const identity = keys.address();
      if (existing) {
        if (
          !equalBytes(existing.nullifierPublicKey, identity.nullifierPublicKey) ||
          !equalBytes(existing.viewingPublicKey, identity.viewingPublicKey.toBytes())
        ) {
          throw new Error(`${name} has registered different privacy keys`);
        }
      } else {
        const ix = await getRegisterInstructionAsync({
          owner: signer,
          nullifierPublicKey: identity.nullifierPublicKey,
          viewingPublicKey: identity.viewingPublicKey.toBytes(),
        });
        const tx = await send(client, signer, [ix]);
        transactions[`${name}Registration`] = { signature: tx.signature, slot: String(tx.slot) };
        const registered = await fetchUserRecord({ rpc: client, owner: signer.address });
        if (
          !registered ||
          !equalBytes(registered.nullifierPublicKey, identity.nullifierPublicKey) ||
          !equalBytes(registered.viewingPublicKey, identity.viewingPublicKey.toBytes())
        ) {
          throw new Error(`${name} registration was not readable after confirmation`);
        }
      }
      report(`${name} registration passed`);
    }

    // Keep the SDK's deposit, proof, and transact calls visible, as in deposit_transfer_withdraw.ts.
    const deposit = await depositInstruction({
      tree: client.tree,
      depositor: senderSigner,
      deposits: [
        {
          asset: DepositAsset.sol(),
          viewTag: sender.identity.confidentialViewTag(),
          recipientOwnerHash: sender.identity.ownerHash(),
          amount: DEPOSIT_AMOUNT,
        },
      ],
    });
    const depositTx = await send(client, senderSigner, [deposit]);
    transactions.deposit = { signature: depositTx.signature, slot: String(depositTx.slot) };
    await sync(sender, senderKeys, depositTx);
    expectBalance(sender, DEPOSIT_AMOUNT);
    report("deposit passed", transactions.deposit);

    const entry = sender.utxos().find((entry) => !entry.spent && entry.utxo.asset === SOL_MINT)!;
    const transfer = new ConfidentialTransfer(
      sender.identity,
      [
        new ProofInputUtxo({
          utxo: entry.utxo,
          nullifierPublicKey: sender.identity.nullifierPublicKey,
          nullifier: entry.nullifier,
        }),
      ],
      senderSigner.address,
    );
    transfer.send(recipient.identity, SOL_MINT, TRANSFER_AMOUNT);
    const prepared = transfer.prepare();
    const [txKey] = await senderKeys.transactionKeys([
      {
        viewingPublicKey: sender.identity.viewingPublicKey,
        firstNullifier: prepared.firstNullifier,
      },
    ]);
    let proofInputs;
    try {
      proofInputs = prepared.finalize(
        encryptConfidentialTransfer(txKey!, { outputs: prepared.outputs, assets: sender.registry }),
      );
    } finally {
      txKey?.destroy();
    }
    const transferData = await client.proveTransact(proofInputs, senderKeys);
    if (transferData.interfaceTransfers.length !== 0)
      throw new Error("Confidential transfer unexpectedly moves public funds");
    const transferIx = await transactInstruction({
      payer: senderSigner,
      ownerSigners: [senderSigner],
      inputTree: client.tree,
      outputTree: client.tree,
      data: transferData,
    });
    const transferTx = await send(client, senderSigner, [transferIx]);
    transactions.transfer = { signature: transferTx.signature, slot: String(transferTx.slot) };
    await sync(sender, senderKeys, transferTx);
    await sync(recipient, recipientKeys, transferTx);
    expectBalance(sender, DEPOSIT_AMOUNT - TRANSFER_AMOUNT);
    expectBalance(recipient, TRANSFER_AMOUNT);
    report("confidential transfer and recipient decryption passed", transactions.transfer);

    for (const [name, wallet, signer, keys, amount] of [
      ["recipient", recipient, recipientSigner, recipientKeys, TRANSFER_AMOUNT],
      ["sender", sender, senderSigner, senderKeys, DEPOSIT_AMOUNT - TRANSFER_AMOUNT],
    ] as const) {
      const note = wallet.utxos().find((entry) => !entry.spent && entry.utxo.asset === SOL_MINT)!;
      const withdrawal = new ConfidentialTransfer(
        wallet.identity,
        [
          new ProofInputUtxo({
            utxo: note.utxo,
            nullifierPublicKey: wallet.identity.nullifierPublicKey,
            nullifier: note.nullifier,
          }),
        ],
        signer.address,
      );
      withdrawal.withdraw(SOL_MINT, amount, WithdrawalTarget.sol({ recipient: signer.address }));
      const preparedWithdrawal = withdrawal.prepare();
      const [withdrawalKey] = await keys.transactionKeys([
        {
          viewingPublicKey: wallet.identity.viewingPublicKey,
          firstNullifier: preparedWithdrawal.firstNullifier,
        },
      ]);
      let withdrawalInputs;
      try {
        withdrawalInputs = preparedWithdrawal.finalize(
          encryptConfidentialTransfer(withdrawalKey!, {
            outputs: preparedWithdrawal.outputs,
            assets: wallet.registry,
          }),
        );
      } finally {
        withdrawalKey?.destroy();
      }
      const data = await client.proveTransact(withdrawalInputs, keys);
      const instruction = await transactInstruction({
        payer: signer,
        ownerSigners: [signer],
        inputTree: client.tree,
        outputTree: client.tree,
        withdrawal: TransactWithdrawal.sol({ recipient: signer.address }),
        data,
      });
      const balanceBefore = await client.getBalance(signer.address);
      const tx = await send(client, signer, [instruction]);
      transactions[`${name}Withdrawal`] = { signature: tx.signature, slot: String(tx.slot) };
      await sync(wallet, keys, tx);
      expectBalance(wallet, 0n);
      const balanceAfter = await client.getBalance(signer.address);
      if (balanceAfter <= balanceBefore)
        throw new Error(`${name} public balance did not increase after withdrawal`);
      report(`${name} withdrawal passed`, transactions[`${name}Withdrawal`]);
    }
    return {
      network: "devnet",
      sender: senderSigner.address,
      recipient: recipientSigner.address,
      activationRecovery: true,
      depositLamports: String(DEPOSIT_AMOUNT),
      transferLamports: String(TRANSFER_AMOUNT),
      senderPrivateBalance: "0",
      recipientPrivateBalance: "0",
      transactions,
    };

    async function sync(wallet: Wallet, keys: LocalKeys, tx: Awaited<ReturnType<typeof send>>) {
      const indexed = await client.getShieldedTransactionsBySignature(
        tx.signature,
        atSlot(tx.slot),
      );
      const result = await decryptTransactions({
        wallet,
        keys,
        transactions: indexed.transactions.map(({ transaction }) => transaction),
      });
      if (
        result.unparsedTransactions > 0 ||
        result.unknownAssetIds.length > 0 ||
        result.unknownAssetFields.length > 0
      ) {
        throw new Error("Indexed transaction could not be fully decrypted");
      }
    }
  } finally {
    senderKeys.destroy();
    recipientKeys.destroy();
    restoredSender.destroy();
  }
}

function expectBalance(wallet: Wallet, amount: bigint) {
  if (wallet.balance(SOL_MINT).amount !== amount) {
    throw new Error(
      `Expected private SOL balance ${amount}, got ${wallet.balance(SOL_MINT).amount}`,
    );
  }
}

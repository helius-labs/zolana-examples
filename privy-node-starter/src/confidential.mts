import "dotenv/config";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  SOL_MINT,
  Wallet,
  createZolanaClient,
  initializePoseidon,
} from "@heliuslabs/zolana";
import {
  LocalKeys,
  atSlot,
  teePolicyFromJson,
} from "@heliuslabs/zolana/client";
import {
  ed25519DerivationMessage,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import {
  DepositAsset,
  TransactWithdrawal,
  depositInstruction,
  transactInstruction,
} from "@heliuslabs/zolana/interface";
import {
  ConfidentialTransfer,
  ProofInputUtxo,
  WithdrawalTarget,
  decryptTransactions,
  encryptConfidentialTransfer,
} from "@heliuslabs/zolana/transaction";
import { getSetComputeUnitLimitInstruction } from "@solana-program/compute-budget";
import {
  address,
  appendTransactionMessageInstructions,
  assertIsFullySignedTransaction,
  assertIsTransactionWithinSizeLimit,
  compileTransaction,
  createTransactionMessage,
  getAddressEncoder,
  getSignatureFromTransaction,
  getTransactionDecoder,
  getTransactionEncoder,
  pipe,
  sendTransactionWithoutConfirmingFactory,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Instruction,
} from "@solana/kit";
import privyClient from "./lib/privy.js";

const { privy } = privyClient;

type Client = Awaited<ReturnType<typeof createZolanaClient>>;
type SolanaWallet = { id: string; address: string };

const DEPOSIT_AMOUNT = 10_000_000n;
const TRANSFER_AMOUNT = 3_000_000n;

async function main() {
  await initializePoseidon();
  const client = await createZolanaClient({
    solanaRpcUrl: process.env.HELIUS_RPC_URL!,
    indexerUrl: "https://d34xe1hqb2ibg2.cloudfront.net",
    proverUrl: "https://dd73hh473h6ha.cloudfront.net",
    proofDataSource: "prover",
    proverTee: teePolicyFromJson(
      JSON.parse(readFileSync("../devnet-e-tee-policy.json", "utf8")),
    ),
  });
  assert.equal(
    await client.solanaRpc.getGenesisHash().send(),
    "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG",
    "Use Solana Devnet",
  );
  const sender = await privy.wallets().get(process.env.SENDER_WALLET_ID!);
  const recipient = await privy.wallets().get(process.env.RECIPIENT_WALLET_ID!);
  const senderKeys = await deriveKeys(client, sender);
  let recipientKeys: LocalKeys | undefined;
  try {
    recipientKeys = await deriveKeys(client, recipient);
    const senderState = new Wallet({ identity: senderKeys.address() });
    const recipientState = new Wallet({ identity: recipientKeys.address() });

    // Deposit public SOL into the sender's private balance.
    const deposit = await depositInstruction({
      tree: client.tree,
      depositor: address(sender.address),
      deposits: [
        {
          asset: DepositAsset.sol(),
          viewTag: senderState.identity.confidentialViewTag(),
          recipientOwnerHash: senderState.identity.ownerHash(),
          amount: DEPOSIT_AMOUNT,
        },
      ],
    });
    const depositTx = await send(client, sender, [deposit]);
    await sync(client, senderState, senderKeys, depositTx);
    assert.equal(senderState.balance(SOL_MINT).amount, DEPOSIT_AMOUNT);
    console.log("deposit", depositTx.signature);

    // Encrypt the transfer outputs, prove the spend, and sign with Privy.
    const entry = senderState
      .utxos()
      .find((entry) => !entry.spent && entry.utxo.asset === SOL_MINT)!;
    const transfer = new ConfidentialTransfer(
      senderState.identity,
      [
        new ProofInputUtxo({
          utxo: entry.utxo,
          nullifierPublicKey: senderState.identity.nullifierPublicKey,
          nullifier: entry.nullifier,
        }),
      ],
      address(sender.address),
    );
    transfer.send(recipientState.identity, SOL_MINT, TRANSFER_AMOUNT);
    const prepared = transfer.prepare();
    const [txKey] = await senderKeys.transactionKeys([
      {
        viewingPublicKey: senderState.identity.viewingPublicKey,
        firstNullifier: prepared.firstNullifier,
      },
    ]);
    let proofInputs;
    try {
      proofInputs = prepared.finalize(
        encryptConfidentialTransfer(txKey!, {
          outputs: prepared.outputs,
          assets: senderState.registry,
        }),
      );
    } finally {
      txKey?.destroy();
    }
    const transferData = await client.proveTransact(proofInputs, senderKeys);
    const transferInstruction = await transactInstruction({
      payer: address(sender.address),
      ownerSigners: [address(sender.address)],
      inputTree: client.tree,
      outputTree: client.tree,
      data: transferData,
    });
    const transferTx = await send(client, sender, [transferInstruction]);
    await sync(client, senderState, senderKeys, transferTx);
    await sync(client, recipientState, recipientKeys, transferTx);
    assert.equal(
      senderState.balance(SOL_MINT).amount,
      DEPOSIT_AMOUNT - TRANSFER_AMOUNT,
    );
    assert.equal(recipientState.balance(SOL_MINT).amount, TRANSFER_AMOUNT);
    console.log("confidential transfer", transferTx.signature);

    // Withdraw both private balances to their owners' public wallets.
    for (const [wallet, state, keys, amount] of [
      [recipient, recipientState, recipientKeys, TRANSFER_AMOUNT],
      [sender, senderState, senderKeys, DEPOSIT_AMOUNT - TRANSFER_AMOUNT],
    ] as const) {
      const note = state
        .utxos()
        .find((entry) => !entry.spent && entry.utxo.asset === SOL_MINT)!;
      const withdrawal = new ConfidentialTransfer(
        state.identity,
        [
          new ProofInputUtxo({
            utxo: note.utxo,
            nullifierPublicKey: state.identity.nullifierPublicKey,
            nullifier: note.nullifier,
          }),
        ],
        address(wallet.address),
      );
      withdrawal.withdraw(
        SOL_MINT,
        amount,
        WithdrawalTarget.sol({ recipient: address(wallet.address) }),
      );
      const preparedWithdrawal = withdrawal.prepare();
      const [withdrawalKey] = await keys.transactionKeys([
        {
          viewingPublicKey: state.identity.viewingPublicKey,
          firstNullifier: preparedWithdrawal.firstNullifier,
        },
      ]);
      let withdrawalInputs;
      try {
        withdrawalInputs = preparedWithdrawal.finalize(
          encryptConfidentialTransfer(withdrawalKey!, {
            outputs: preparedWithdrawal.outputs,
            assets: state.registry,
          }),
        );
      } finally {
        withdrawalKey?.destroy();
      }
      const data = await client.proveTransact(withdrawalInputs, keys);
      const instruction = await transactInstruction({
        payer: address(wallet.address),
        ownerSigners: [address(wallet.address)],
        inputTree: client.tree,
        outputTree: client.tree,
        withdrawal: TransactWithdrawal.sol({
          recipient: address(wallet.address),
        }),
        data,
      });
      const tx = await send(client, wallet, [instruction]);
      await sync(client, state, keys, tx);
      assert.equal(state.balance(SOL_MINT).amount, 0n);
      console.log("withdraw", wallet.address, tx.signature);
    }
  } finally {
    senderKeys.destroy();
    recipientKeys?.destroy();
  }
}

async function deriveKeys(client: Client, wallet: SolanaWallet) {
  const owner = address(wallet.address);
  const publicKey = new Uint8Array(
    getAddressEncoder().encode(owner),
  ) as Bytes32;
  const message = ed25519DerivationMessage(publicKey);
  const result = await privy
    .wallets()
    .solana()
    .signMessage(wallet.id, { message });
  const seed = new Uint8Array(Buffer.from(result.signature, "base64"));
  try {
    return LocalKeys.fromDerivationSeed(
      { solanaPublicKey: owner, derivationSeed: seed },
      client.proofService,
    );
  } finally {
    seed.fill(0);
  }
}

async function send(
  client: Client,
  wallet: SolanaWallet,
  instructions: readonly Instruction[],
) {
  const { value: lifetime } = await client.solanaRpc
    .getLatestBlockhash()
    .send();
  const transaction = compileTransaction(
    pipe(
      createTransactionMessage({ version: 0 }),
      (message) =>
        setTransactionMessageFeePayer(address(wallet.address), message),
      (message) =>
        setTransactionMessageLifetimeUsingBlockhash(lifetime, message),
      (message) =>
        appendTransactionMessageInstructions(
          [
            getSetComputeUnitLimitInstruction({ units: 450_000 }),
            ...instructions,
          ],
          message,
        ),
    ),
  );
  const result = await privy
    .wallets()
    .solana()
    .signTransaction(wallet.id, {
      transaction: new Uint8Array(getTransactionEncoder().encode(transaction)),
    });
  const decoded = getTransactionDecoder().decode(
    Buffer.from(result.signed_transaction, "base64"),
  );
  const signed = { ...transaction, signatures: decoded.signatures };
  assertIsFullySignedTransaction(signed);
  assertIsTransactionWithinSizeLimit(signed);
  await sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc })(
    signed,
    { commitment: "confirmed" },
  );
  const signature = getSignatureFromTransaction(signed);
  const slot = await client.confirmTransaction(signature);
  return { signature, slot };
}

async function sync(
  client: Client,
  wallet: Wallet,
  keys: LocalKeys,
  tx: Awaited<ReturnType<typeof send>>,
) {
  const indexed = await client.getShieldedTransactionsBySignature(
    tx.signature,
    atSlot(tx.slot),
  );
  await decryptTransactions({
    wallet,
    keys,
    transactions: indexed.transactions.map(({ transaction }) => transaction),
  });
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

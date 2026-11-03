import { RequestHandler } from "express";
import { SOL_MINT, Wallet } from "@heliuslabs/zolana";
import { transactInstruction } from "@heliuslabs/zolana/interface";
import {
  ConfidentialTransfer,
  ProofInputUtxo,
  encryptConfidentialTransfer,
} from "@heliuslabs/zolana/transaction";
import {
  resolveRegisteredAddress,
  syncWallet,
} from "@heliuslabs/zolana/wallet";
import { address, signature } from "@solana/kit";
import {
  getClient,
  loadSender,
  encodeTransaction,
  readAmount,
  privy,
  SENDER_WALLET_ID,
  TREASURY_AUTHORIZATION_KEY,
} from "../../confidential.mjs";

export const transfer: RequestHandler = async (req, res) => {
  const mint = address(req.body.mint ?? SOL_MINT);
  const amount = readAmount(req.body.amount);
  const client = await getClient();
  const recipient = await resolveRegisteredAddress({
    rpc: client,
    owner: address(req.body.recipient),
  });
  if (!recipient) throw new Error("Recipient is not registered");
  const { owner, keys } = await loadSender(client);
  try {
    const wallet = new Wallet({ identity: keys.address() });
    await syncWallet({
      wallet,
      keys,
      client,
      config: {
        requireSlot: await client.solanaRpc
          .getSlot({ commitment: "confirmed" })
          .send(),
      },
    });
    const candidates = wallet
      .utxos()
      .filter(
        (entry) =>
          !entry.spent &&
          entry.utxo.asset === mint &&
          entry.outputContext.tree === client.tree &&
          entry.utxo.ringProgramId === undefined &&
          entry.ringDataHash === undefined &&
          entry.dataHash === undefined &&
          entry.utxo.data.isEmpty(),
      )
      .sort((a, b) =>
        a.utxo.amount > b.utxo.amount
          ? -1
          : a.utxo.amount < b.utxo.amount
            ? 1
            : 0,
      );
    const inputs: ProofInputUtxo[] = [];
    let available = 0n;
    for (const entry of candidates) {
      inputs.push(
        new ProofInputUtxo({
          utxo: entry.utxo,
          nullifierPublicKey: wallet.identity.nullifierPublicKey,
          nullifier: entry.nullifier,
        }),
      );
      available += entry.utxo.amount;
      if (available >= amount) break;
    }
    if (available < amount) throw new Error("Insufficient private balance");

    const transfer = new ConfidentialTransfer(wallet.identity, inputs, owner);
    transfer.send(recipient.address, mint, amount);
    const prepared = transfer.prepare();
    const [txKey] = await keys.transactionKeys([
      {
        viewingPublicKey: wallet.identity.viewingPublicKey,
        firstNullifier: prepared.firstNullifier,
      },
    ]);
    let proofInputs;
    try {
      proofInputs = prepared.finalize(
        encryptConfidentialTransfer(txKey!, {
          outputs: prepared.outputs,
          assets: wallet.registry,
        }),
      );
    } finally {
      txKey?.destroy();
    }
    const data = await client.proveTransact(proofInputs, keys);

    const instruction = await transactInstruction({
      payer: owner,
      ownerSigners: [owner],
      inputTree: client.tree,
      outputTree: client.tree,
      data,
    });
    const transaction = await encodeTransaction(client, owner, [instruction]);
    const sentTransaction = await privy
      .wallets()
      .solana()
      .signAndSendTransaction(SENDER_WALLET_ID, {
        caip2: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
        transaction,
        authorization_context: {
          authorization_private_keys: [TREASURY_AUTHORIZATION_KEY],
        },
      });
    await client.confirmTransaction(signature(sentTransaction.hash));
    res.status(200).json({ hash: sentTransaction.hash });
  } finally {
    keys.destroy();
  }
};

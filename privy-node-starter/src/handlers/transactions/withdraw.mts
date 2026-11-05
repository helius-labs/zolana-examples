import { RequestHandler } from "express";
import { SOL_MINT, Wallet } from "@heliuslabs/zolana";
import { getAssociatedTokenAddress } from "@heliuslabs/zolana/addresses";
import { getCreateAssociatedTokenAccountInstructionAsync } from "@heliuslabs/zolana/instructions";
import {
  SHIELDED_POOL_PROGRAM_ID,
  TransactWithdrawal,
  transactInstruction,
} from "@heliuslabs/zolana/interface";
import {
  ConfidentialTransfer,
  ProofInputUtxo,
  WithdrawalTarget,
  encryptConfidentialTransfer,
} from "@heliuslabs/zolana/transaction";
import { syncWallet } from "@heliuslabs/zolana/wallet";
import { TOKEN_PROGRAM_ADDRESS } from "@solana-program/token";
import {
  address,
  getAddressEncoder,
  getProgramDerivedAddress,
  signature,
  type Instruction,
} from "@solana/kit";
import {
  getClient,
  loadSender,
  encodeTransaction,
  readAmount,
  privy,
  SENDER_WALLET_ID,
  TREASURY_AUTHORIZATION_KEY,
} from "../../confidential.mjs";

export const withdraw: RequestHandler = async (req, res) => {
  const mint = address(req.body.mint ?? SOL_MINT);
  const amount = readAmount(req.body.amount);
  const recipient = address(req.body.recipient);
  const client = await getClient();
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

    const instructions: Instruction[] = [];
    let target: WithdrawalTarget = WithdrawalTarget.sol({ recipient });
    let withdrawal: TransactWithdrawal = TransactWithdrawal.sol({ recipient });
    if (mint !== SOL_MINT) {
      const recipientTokenAccount = await getAssociatedTokenAddress(
        recipient,
        mint,
        TOKEN_PROGRAM_ADDRESS,
      );
      const [splTokenInterface, splInterfaceBump] =
        await getProgramDerivedAddress({
          programAddress: SHIELDED_POOL_PROGRAM_ID,
          seeds: [
            new TextEncoder().encode("spl_asset_vault"),
            getAddressEncoder().encode(mint),
          ],
        });
      target = WithdrawalTarget.spl({
        recipientTokenAccount,
        splTokenInterface,
        splInterfaceBump,
      });
      withdrawal = TransactWithdrawal.spl({
        mint,
        splTokenInterface,
        recipientTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
      });
      instructions.push(
        await getCreateAssociatedTokenAccountInstructionAsync({
          payer: owner,
          owner: recipient,
          mint,
          tokenProgram: TOKEN_PROGRAM_ADDRESS,
        }),
      );
    }
    const transfer = new ConfidentialTransfer(wallet.identity, inputs, owner);
    transfer.withdraw(mint, amount, target);
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

    instructions.push(
      await transactInstruction({
        payer: owner,
        ownerSigners: [owner],
        inputTree: client.tree,
        outputTree: client.tree,
        withdrawal,
        data,
      }),
    );
    const transaction = await encodeTransaction(client, owner, instructions);
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

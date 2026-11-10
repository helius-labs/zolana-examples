import { RequestHandler } from "express";
import { SOL_MINT } from "@heliuslabs/zolana";
import {
  getAssociatedTokenAddress,
  getSplAssetVaultAddress,
} from "@heliuslabs/zolana/addresses";
import { getCreateSplInterfaceInstructionAsync } from "@heliuslabs/zolana/instructions";
import { DepositAsset, depositInstruction } from "@heliuslabs/zolana/interface";
import { TOKEN_PROGRAM_ADDRESS } from "@solana-program/token";
import { address, signature, type Instruction } from "@solana/kit";
import {
  getClient,
  loadSender,
  encodeTransaction,
  readAmount,
  privy,
  SENDER_WALLET_ID,
  TREASURY_AUTHORIZATION_KEY,
} from "../../confidential.mjs";

export const deposit: RequestHandler = async (req, res) => {
  const mint = address(req.body.mint ?? SOL_MINT);
  const amount = readAmount(req.body.amount);
  const client = await getClient();
  const { owner, keys } = await loadSender(client);
  try {
    const instructions: Instruction[] = [];
    let asset: DepositAsset = DepositAsset.sol();
    if (mint !== SOL_MINT) {
      if (!(await client.getAccount(await getSplAssetVaultAddress(mint)))) {
        instructions.push(
          await getCreateSplInterfaceInstructionAsync({
            authority: owner,
            mint,
            tokenProgram: TOKEN_PROGRAM_ADDRESS,
          }),
        );
      }
      asset = DepositAsset.spl({
        mint,
        sourceTokenAccount: await getAssociatedTokenAddress(
          owner,
          mint,
          TOKEN_PROGRAM_ADDRESS,
        ),
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
      });
    }
    const identity = keys.address();
    instructions.push(
      await depositInstruction({
        tree: client.tree,
        depositor: owner,
        deposits: [
          {
            asset,
            viewTag: identity.confidentialViewTag(),
            recipientOwnerHash: identity.ownerHash(),
            amount,
          },
        ],
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

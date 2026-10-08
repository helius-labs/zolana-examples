import { getTransferSolInstruction } from "@solana-program/system";
import {
  TOKEN_PROGRAM_ADDRESS,
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstruction,
  getTransferInstruction,
} from "@solana-program/token";
import {
  appendTransactionMessageInstructions,
  compileTransaction,
  createTransactionMessage,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  type Address,
  type Instruction,
  type Transaction,
  type TransactionSigner,
} from "@solana/kit";
import { zolana } from "./private-wallet.js";

/** One transaction of `instructions`, paid for by `payer`, ready to sign. */
async function transaction(
  payer: TransactionSigner,
  instructions: readonly Instruction[],
): Promise<Transaction> {
  const client = await zolana;
  const { value: blockhash } = await client.solanaRpc
    .getLatestBlockhash({ commitment: client.commitment })
    .send();
  return pipe(
    createTransactionMessage({ version: 0 }),
    (message) => setTransactionMessageFeePayerSigner(payer, message),
    (message) =>
      setTransactionMessageLifetimeUsingBlockhash(blockhash, message),
    (message) => appendTransactionMessageInstructions(instructions, message),
    compileTransaction,
  );
}

/** A public SOL transfer from the wallet. */
export function publicSolTransfer(
  wallet: TransactionSigner,
  recipient: Address,
  lamports: bigint,
): Promise<Transaction> {
  return transaction(wallet, [
    getTransferSolInstruction({
      source: wallet,
      destination: recipient,
      amount: lamports,
    }),
  ]);
}

/**
 * A public SPL transfer from the wallet's token account to the recipient's,
 * which it creates when missing. Works for Token and Token-2022 mints.
 */
export async function publicSplTransfer(
  wallet: TransactionSigner,
  recipient: Address,
  mint: Address,
  amount: bigint,
): Promise<Transaction> {
  const client = await zolana;
  const { value: mintAccount } = await client.solanaRpc
    .getAccountInfo(mint, { encoding: "base64" })
    .send();
  if (!mintAccount) throw new Error(`no mint ${mint}`);
  const tokenProgram = mintAccount.owner;
  const [source] = await findAssociatedTokenPda({
    mint,
    owner: wallet.address,
    tokenProgram,
  });
  const [destination] = await findAssociatedTokenPda({
    mint,
    owner: recipient,
    tokenProgram,
  });
  return transaction(wallet, [
    getCreateAssociatedTokenIdempotentInstruction({
      payer: wallet,
      ata: destination,
      owner: recipient,
      mint,
      tokenProgram,
    }),
    getTransferInstruction(
      { source, destination, authority: wallet, amount },
      { programAddress: tokenProgram as typeof TOKEN_PROGRAM_ADDRESS },
    ),
  ]);
}

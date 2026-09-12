import {
  appendTransactionMessageInstructions,
  compileTransaction,
  createTransactionMessage,
  pipe,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Instruction,
} from "@solana/kit";
import { getSetComputeUnitLimitInstruction } from "@solana-program/compute-budget";
import { checkedTransactionSize } from "@heliuslabs/zolana/interface";
import type { PublicWalletContext } from "./walletContext";

export async function compileInstructions(
  ctx: PublicWalletContext,
  instructions: readonly Instruction[],
  privateSpend = false,
) {
  ctx.assertActive();
  const lifetime = await ctx.client.getLatestBlockhash({ signal: ctx.signal });
  ctx.assertActive();
  return checkedTransactionSize(
    compileTransaction(
      pipe(
        createTransactionMessage({ version: 0 }),
        (tx) => setTransactionMessageFeePayer(ctx.owner, tx),
        (tx) => setTransactionMessageLifetimeUsingBlockhash(lifetime, tx),
        (tx) =>
          appendTransactionMessageInstructions(
            [
              ...(privateSpend
                ? [getSetComputeUnitLimitInstruction({ units: 300_000 })]
                : []),
              ...instructions,
            ],
            tx,
          ),
      ),
    ),
  );
}

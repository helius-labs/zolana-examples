import { address, type Address } from "@solana/kit";
import { SOL_MINT } from "@heliuslabs/zolana";
import {
  ConfidentialTransfer,
  WithdrawalTarget,
} from "@heliuslabs/zolana/transaction";
import {
  getTransactInstruction,
  TransactWithdrawal,
} from "@heliuslabs/zolana/instructions";
import { assertLamports } from "../../lib/parseSol";
import type { PrivateWalletContext } from "../../lib/walletContext";
import { readPrivateState } from "../../lib/readPrivateState";
import { selectSolInputs, proveSpend } from "../../lib/privateSpend";
import { compileInstructions } from "../../lib/compileInstructions";
import { syncAfterTransaction } from "../../lib/syncAfterTransaction";
import { WITHDRAW_AMOUNT } from "../../lib/amounts";

export async function withdrawSol(
  ctx: PrivateWalletContext,
  amount = WITHDRAW_AMOUNT,
  recipient: Address = ctx.owner,
) {
  ctx.assertActive();
  assertLamports(amount);
  const destination = address(recipient);
  const state = await readPrivateState(ctx);
  ctx.assertActive();
  const transfer = new ConfidentialTransfer(
    ctx.keys.address(),
    selectSolInputs(ctx, state.notes, amount),
    ctx.owner,
  );
  transfer.withdraw(
    SOL_MINT,
    amount,
    WithdrawalTarget.sol({ recipient: destination }),
  );
  const data = await proveSpend(ctx, transfer.prepare(), state.registry, {
    amount,
    withdrawalRecipient: destination,
  });
  const instruction = getTransactInstruction({
    payer: ctx.owner,
    inputTree: ctx.client.tree,
    outputTree: ctx.client.tree,
    data,
    withdrawal: TransactWithdrawal.sol({ recipient: destination }),
  });
  const tx = await compileInstructions(ctx, [instruction], true);
  ctx.assertActive();
  const { signature, slot } = await ctx.submit(tx);
  const privateBalance = await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance };
}

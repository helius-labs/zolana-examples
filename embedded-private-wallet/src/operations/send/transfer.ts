import { SOL_MINT } from "@heliuslabs/zolana";
import { address, type Address } from "@solana/kit";
import { resolveRegisteredAddress } from "@heliuslabs/zolana/wallet";
import { ConfidentialTransfer } from "@heliuslabs/zolana/transaction";
import { getTransactInstruction } from "@heliuslabs/zolana/instructions";
import { assertLamports } from "../../lib/parseSol";
import type { PrivateWalletContext } from "../../lib/walletContext";
import { readPrivateState } from "../../lib/readPrivateState";
import { selectSolInputs, proveSpend } from "../../lib/privateSpend";
import { compileInstructions } from "../../lib/compileInstructions";
import { syncAfterTransaction } from "../../lib/syncAfterTransaction";
import { TRANSFER_AMOUNT } from "../../lib/amounts";
import type { TransferProgressCallback } from "../../lib/transferProgress";

export async function transferSol(
  ctx: PrivateWalletContext,
  recipient: Address,
  amount = TRANSFER_AMOUNT,
  onProgress?: TransferProgressCallback,
) {
  ctx.assertActive();
  assertLamports(amount);
  const destination = address(recipient);
  const report: TransferProgressCallback = (stage, signature) => {
    ctx.assertActive();
    onProgress?.(stage, signature);
  };
  report("preparing");
  const registered = await resolveRegisteredAddress(
    { rpc: ctx.client, owner: destination },
    { signal: ctx.signal },
  );
  ctx.assertActive();
  if (!registered)
    throw new Error("Recipient has not registered a private wallet.");
  const state = await readPrivateState(ctx);
  ctx.assertActive();
  const transfer = new ConfidentialTransfer(
    ctx.keys.address(),
    selectSolInputs(ctx, state.notes, amount),
    ctx.owner,
  );
  transfer.send(registered.address, SOL_MINT, amount);
  const data = await proveSpend(
    ctx,
    transfer.prepare(),
    state.registry,
    { amount, recipient: registered.address },
    () => report("proving"),
  );
  const instruction = getTransactInstruction({
    payer: ctx.owner,
    inputTree: ctx.client.tree,
    outputTree: ctx.client.tree,
    data,
  });
  const tx = await compileInstructions(ctx, [instruction], true);
  report("signing");
  const { signature, slot } = await ctx.submit(tx, () => report("sending"));
  report("confirmed", signature);
  const privateBalance = await syncAfterTransaction(ctx, signature, slot);
  report("done", signature);
  return { signature, privateBalance };
}

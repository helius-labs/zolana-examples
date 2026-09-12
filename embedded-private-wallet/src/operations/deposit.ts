import { SOL_MINT } from "@heliuslabs/zolana";
import { assertLamports } from "../lib/parseSol";
import type { PrivateWalletContext } from "../lib/walletContext";
import { syncAfterTransaction } from "../lib/syncAfterTransaction";
import { buildDepositTransaction } from "@heliuslabs/zolana";
import { DEPOSIT_AMOUNT } from "../lib/amounts";

export async function depositSol(
  ctx: PrivateWalletContext,
  amount = DEPOSIT_AMOUNT
) {
  const { client, wallet, keys, owner, submit } = ctx;
  ctx.assertActive();
  assertLamports(amount);
  const tx = await buildDepositTransaction(
    {
      client,
      feePayer: owner,
      recipient: keys.address(),
      amount,
    },
    { signal: ctx.signal }
  );
  ctx.assertActive();
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}

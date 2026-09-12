import { SOL_MINT } from "@heliuslabs/zolana";
import { assertLamports } from "../lib/parseSol";
import type { PrivateWalletContext } from "../lib/walletContext";
import { syncAfterTransaction } from "../lib/syncAfterTransaction";
import { buildWithdrawalTransaction } from "@heliuslabs/zolana";
import { WITHDRAW_AMOUNT } from "../lib/amounts";

export async function withdrawSol(
  ctx: PrivateWalletContext,
  amount = WITHDRAW_AMOUNT
) {
  const { client, wallet, keys, owner, submit } = ctx;
  ctx.assertActive();
  assertLamports(amount);
  const tx = await buildWithdrawalTransaction(
    {
      client,
      wallet,
      keys,
      feePayer: owner,
      recipient: owner,
      amount,
    },
    { signal: ctx.signal }
  );
  ctx.assertActive();
  const { signature, slot } = await submit(tx);
  await syncAfterTransaction(ctx, signature, slot);
  return { signature, privateBalance: wallet.balance(SOL_MINT).amount };
}

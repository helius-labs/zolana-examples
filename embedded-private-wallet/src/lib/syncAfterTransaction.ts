import type { PrivateWalletContext } from "./walletContext";
import { getPrivateSolBalance } from "../operations/read/getBalance";

export class BalanceSyncError extends Error {
  constructor(readonly signature: string) {
    super(
      "Transaction confirmed, but balances could not refresh. Refresh balances before making another transaction.",
    );
    this.name = "BalanceSyncError";
  }
}

export async function syncAfterTransaction(
  ctx: PrivateWalletContext,
  signature: string,
  slot: bigint,
) {
  try {
    ctx.assertActive();
    const balance = await getPrivateSolBalance(ctx, { requireSlot: slot });
    ctx.assertActive();
    return balance;
  } catch {
    throw new BalanceSyncError(signature);
  }
}

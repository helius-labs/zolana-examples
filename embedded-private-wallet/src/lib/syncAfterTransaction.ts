import type { PrivateWalletContext } from "./walletContext";
import { syncPrivateWallet } from "../operations/syncWallet";

export class BalanceSyncError extends Error {
  constructor(readonly signature: string) {
    super(
      "Transaction confirmed, but balances could not refresh. Refresh balances before making another transaction."
    );
    this.name = "BalanceSyncError";
  }
}

export async function syncAfterTransaction(
  ctx: PrivateWalletContext,
  signature: string,
  slot: bigint
) {
  try {
    ctx.assertActive();
    await syncPrivateWallet(ctx, { requireSlot: slot });
    ctx.assertActive();
  } catch {
    throw new BalanceSyncError(signature);
  }
}

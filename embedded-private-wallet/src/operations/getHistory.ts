import { getPrivateTransactions } from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../lib/walletContext";

/** Reads the history reconstructed by the last sync; no signing or RPC call. */
export function getPrivateHistory(
  ctx: Pick<PrivateWalletContext, "wallet" | "assertActive">
) {
  ctx.assertActive();
  return getPrivateTransactions(ctx.wallet);
}

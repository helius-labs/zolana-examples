import type { SyncWalletConfig } from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../lib/walletContext";
import { syncPrivateWallet } from "./syncWallet";
import { getPrivateHistory } from "./getHistory";

/** Fetches new indexed activity, updates wallet state, and returns its history. */
export async function syncPrivateHistory(
  ctx: PrivateWalletContext,
  config?: SyncWalletConfig,
  signal: AbortSignal = ctx.signal
) {
  await syncPrivateWallet(ctx, config, signal);
  return getPrivateHistory(ctx);
}

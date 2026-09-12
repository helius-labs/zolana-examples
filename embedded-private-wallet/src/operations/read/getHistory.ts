import { readPrivateState, type ReadOptions } from "../../lib/readPrivateState";
import type { PrivateWalletContext } from "../../lib/walletContext";

/** Full indexer read; returns confirmed private history, including spent notes. */
export async function getPrivateHistory(
  ctx: PrivateWalletContext,
  options?: ReadOptions,
) {
  ctx.assertActive();
  const result = await readPrivateState(ctx, options);
  ctx.assertActive();
  return result.history;
}

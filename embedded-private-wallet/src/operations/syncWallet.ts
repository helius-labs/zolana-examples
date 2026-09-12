import { syncWallet, type SyncWalletConfig } from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../lib/walletContext";

/** Updates both private balances and history. Reuses the active TVC keys. */
export async function syncPrivateWallet(
  ctx: Pick<
    PrivateWalletContext,
    "client" | "wallet" | "keys" | "assertActive" | "signal"
  >,
  config?: SyncWalletConfig,
  signal: AbortSignal = ctx.signal
) {
  ctx.assertActive();
  const report = await syncWallet(
    {
      client: ctx.client,
      wallet: ctx.wallet,
      keys: ctx.keys,
      ...(config ? { config } : {}),
    },
    {
      signal:
        signal === ctx.signal ? signal : AbortSignal.any([ctx.signal, signal]),
    }
  );
  ctx.assertActive();
  return report;
}

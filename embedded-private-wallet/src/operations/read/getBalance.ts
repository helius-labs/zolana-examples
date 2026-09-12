import { SOL_MINT } from "@heliuslabs/zolana";
import { address } from "@solana/kit";
import { connectClient } from "../../lib/client";
import { readPrivateState, type ReadOptions } from "../../lib/readPrivateState";
import type { PrivateWalletContext } from "../../lib/walletContext";

/** Public SOL remains available before TVC activation. Amounts stay bigint. */
export async function getPublicSolBalance(
  owner: string,
  client?: Pick<PrivateWalletContext["client"], "getBalance">,
  signal?: AbortSignal,
) {
  signal?.throwIfAborted();
  const rpc = client ?? (await connectClient());
  signal?.throwIfAborted();
  const balance = await rpc.getBalance(address(owner), { signal });
  signal?.throwIfAborted();
  return balance;
}

/** Full indexer read and TVC decryption; no cached wallet balance. */
export async function getPrivateSolBalance(
  ctx: PrivateWalletContext,
  options?: ReadOptions,
) {
  ctx.assertActive();
  const result = await readPrivateState(ctx, options);
  ctx.assertActive();
  return (
    result.balances.find((balance) => balance.mint === SOL_MINT)?.amount ?? 0n
  );
}

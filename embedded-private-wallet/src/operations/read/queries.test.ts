import { beforeEach, expect, it, vi } from "vitest";
import { SOL_MINT } from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../../lib/walletContext";
import { readPrivateState } from "../../lib/readPrivateState";
import { getPublicSolBalance, getPrivateSolBalance } from "./getBalance";
import { getPrivateHistory } from "./getHistory";
vi.mock("../../lib/readPrivateState", () => ({ readPrivateState: vi.fn() }));
const controller = new AbortController();
const ctx = {
  signal: controller.signal,
  assertActive: () => controller.signal.throwIfAborted(),
} as PrivateWalletContext;
beforeEach(() => vi.resetAllMocks());
it("reads bigint public SOL through the SDK independently of private reads", async () => {
  const client = {
    getBalance: vi.fn().mockResolvedValue(9_007_199_254_740_993n),
  };
  expect(await getPublicSolBalance(SOL_MINT, client)).toBe(
    9_007_199_254_740_993n,
  );
  expect(readPrivateState).not.toHaveBeenCalled();
});
it("performs a fresh read for balance and history, forwarding freshness options", async () => {
  const history = [
    { id: { signature: "same", index: 1n } },
    { id: { signature: "same", index: 2n } },
  ];
  vi.mocked(readPrivateState).mockResolvedValue({
    balances: [{ mint: SOL_MINT, amount: 10n }],
    history,
  } as never);
  expect(await getPrivateSolBalance(ctx, { requireSlot: 42n })).toBe(10n);
  expect(await getPrivateHistory(ctx)).toEqual(history);
  expect(readPrivateState).toHaveBeenCalledTimes(2);
  expect(readPrivateState).toHaveBeenCalledWith(ctx, { requireSlot: 42n });
});
it("does not present a failed read as an empty wallet", async () => {
  vi.mocked(readPrivateState).mockRejectedValue(
    new Error("Indexer unavailable"),
  );
  await expect(getPrivateSolBalance(ctx)).rejects.toThrow(
    "Indexer unavailable",
  );
  await expect(getPrivateHistory(ctx)).rejects.toThrow("Indexer unavailable");
});
it("discards results after the caller's session is invalidated", async () => {
  const session = new AbortController();
  vi.mocked(readPrivateState).mockImplementation(async () => {
    session.abort(new Error("Wallet changed"));
    return { balances: [], history: [] } as never;
  });
  await expect(
    getPrivateHistory({
      ...ctx,
      assertActive: () => session.signal.throwIfAborted(),
    }),
  ).rejects.toThrow("Wallet changed");
});

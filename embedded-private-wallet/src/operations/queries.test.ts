import { beforeEach, expect, it, vi } from "vitest";
import { getPrivateTransactions, syncWallet } from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../lib/walletContext";
import { getPublicSolBalance, getPrivateSolBalance } from "./getBalance";
import { getPrivateHistory } from "./getHistory";
import { syncPrivateHistory } from "./syncHistory";
import { syncPrivateWallet } from "./syncWallet";

vi.mock("@heliuslabs/zolana", () => ({
  SOL_MINT: "sol",
  getPrivateTransactions: vi.fn(),
  syncWallet: vi.fn(),
}));
let controller: AbortController;
let ctx: PrivateWalletContext;
beforeEach(() => {
  vi.resetAllMocks();
  controller = new AbortController();
  ctx = {
    wallet: { balance: vi.fn(() => ({ amount: 1_234_567_890n })) },
    client: {},
    keys: {},
    signal: controller.signal,
    assertActive: () => controller.signal.throwIfAborted(),
  } as unknown as PrivateWalletContext;
});
it("reads public and cached private SOL independently without a private sync", async () => {
  const connection = { getBalance: vi.fn().mockResolvedValue(1_234_567_890) };
  expect(
    await getPublicSolBalance(connection, "11111111111111111111111111111111")
  ).toBe(1_234_567_890n);
  expect(getPrivateSolBalance(ctx)).toBe(1_234_567_890n);
  expect(syncWallet).not.toHaveBeenCalled();
});
it("refuses imprecise public lamports", async () => {
  const connection = {
    getBalance: vi.fn().mockResolvedValue(Number.MAX_SAFE_INTEGER + 1),
  };
  await expect(
    getPublicSolBalance(connection, "11111111111111111111111111111111")
  ).rejects.toThrow("precisely");
});
it("returns cached history without signing, syncing, or collapsing rows with the same signature", () => {
  const rows = [
    { id: { signature: "same", index: 0n } },
    { id: { signature: "same", index: 1n } },
  ];
  vi.mocked(getPrivateTransactions).mockReturnValue(rows as never);
  expect(getPrivateHistory(ctx)).toBe(rows);
  expect(syncWallet).not.toHaveBeenCalled();
});
it("syncs before reading history and forwards the required indexer slot", async () => {
  let synced = false;
  vi.mocked(syncWallet).mockImplementation(async () => {
    synced = true;
    return {} as never;
  });
  vi.mocked(getPrivateTransactions).mockImplementation(() => {
    expect(synced).toBe(true);
    return [];
  });
  expect(await syncPrivateHistory(ctx, { requireSlot: 42n })).toEqual([]);
  expect(syncWallet).toHaveBeenCalledWith(
    {
      client: ctx.client,
      wallet: ctx.wallet,
      keys: ctx.keys,
      config: { requireSlot: 42n },
    },
    { signal: ctx.signal }
  );
});
it("does not present cached history as fresh if the indexer fails", async () => {
  vi.mocked(syncWallet).mockRejectedValue(new Error("Indexer unavailable"));
  await expect(syncPrivateHistory(ctx)).rejects.toThrow("Indexer unavailable");
  expect(getPrivateTransactions).not.toHaveBeenCalled();
});
it("discards a sync result after the session is canceled", async () => {
  vi.mocked(syncWallet).mockImplementation(async () => {
    controller.abort(new Error("Wallet changed"));
    return {} as never;
  });
  await expect(syncPrivateHistory(ctx)).rejects.toThrow("Wallet changed");
  expect(getPrivateTransactions).not.toHaveBeenCalled();
  expect(() => getPrivateSolBalance(ctx)).toThrow("Wallet changed");
  expect(() => getPrivateHistory(ctx)).toThrow("Wallet changed");
});
it("keeps session cancellation attached when a caller adds its own timeout signal", async () => {
  const timeout = new AbortController();
  vi.mocked(syncWallet).mockImplementation(async (_input, request) => {
    expect(request?.signal).not.toBe(ctx.signal);
    controller.abort(new Error("Wallet changed"));
    expect(request?.signal?.aborted).toBe(true);
    return {} as never;
  });
  await expect(
    syncPrivateWallet(ctx, undefined, timeout.signal)
  ).rejects.toThrow("Wallet changed");
});

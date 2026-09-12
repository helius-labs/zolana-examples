import { beforeEach, describe, expect, it, vi } from "vitest";
import { address } from "@solana/kit";
import {
  buildDepositTransaction,
  buildTransferTransaction,
  buildWithdrawalTransaction,
  syncWallet,
} from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../lib/walletContext";
import { BalanceSyncError } from "../lib/syncAfterTransaction";
import { depositSol } from "./deposit";
import { transferSol } from "./transfer";
import { withdrawSol } from "./withdraw";
vi.mock("@heliuslabs/zolana", () => ({
  SOL_MINT: "sol",
  syncWallet: vi.fn(),
  buildDepositTransaction: vi.fn(),
  buildTransferTransaction: vi.fn(),
  buildWithdrawalTransaction: vi.fn(),
}));
beforeEach(() => {
  vi.clearAllMocks();
  for (const builder of [
    buildDepositTransaction,
    buildTransferTransaction,
    buildWithdrawalTransaction,
  ]) {
    vi.mocked(builder).mockResolvedValue({} as never);
  }
  vi.mocked(syncWallet).mockRejectedValue(new Error("Indexer unavailable"));
});
describe("confirmed transaction receipts", () => {
  it.each(["deposit", "transfer", "withdraw"])(
    "retains the %s receipt if the indexer fails after confirmation",
    async (action) => {
      const owner = address("11111111111111111111111111111111");
      const ctx = {
        owner,
        keys: { address: () => ({}) },
        assertActive: () => {},
        signal: new AbortController().signal,
        wallet: { balance: () => ({ amount: 1n }) },
        submit: vi
          .fn()
          .mockResolvedValue({ signature: "confirmed", slot: 42n }),
        client: {},
      } as unknown as PrivateWalletContext;
      const result =
        action === "deposit"
          ? depositSol(ctx)
          : action === "transfer"
          ? transferSol(ctx, owner)
          : withdrawSol(ctx);
      await expect(result).rejects.toMatchObject({
        name: "BalanceSyncError",
        signature: "confirmed",
      });
      expect(new BalanceSyncError("confirmed").message).toContain(
        "Transaction confirmed"
      );
      expect(ctx.submit).toHaveBeenCalledTimes(1);
      expect(syncWallet).toHaveBeenCalledWith(
        expect.objectContaining({ config: { requireSlot: 42n } }),
        expect.objectContaining({ signal: ctx.signal })
      );
    }
  );
});

it.each(["deposit", "transfer", "withdraw"])(
  "forwards the selected %s amount to the builder",
  async (action) => {
    vi.mocked(syncWallet).mockResolvedValue(undefined as never);
    const owner = address("11111111111111111111111111111111");
    const ctx = {
      owner,
      keys: { address: () => ({}) },
      assertActive: () => {},
      signal: new AbortController().signal,
      wallet: { balance: () => ({ amount: 0n }) },
      client: {},
      submit: vi.fn().mockResolvedValue({ signature: "receipt", slot: 1n }),
    } as unknown as PrivateWalletContext;
    const amount = 1_234_567n;
    if (action === "deposit") await depositSol(ctx, amount);
    else if (action === "transfer") await transferSol(ctx, owner, amount);
    else await withdrawSol(ctx, amount);
    const builder =
      action === "deposit"
        ? buildDepositTransaction
        : action === "transfer"
        ? buildTransferTransaction
        : buildWithdrawalTransaction;
    expect(builder).toHaveBeenCalledWith(
      expect.objectContaining({ amount }),
      expect.anything()
    );
    await expect(depositSol(ctx, 0n)).rejects.toThrow("greater than zero");
  }
);

it("reports proving at the key-holder boundary and confirmation before balance sync", async () => {
  const report = vi.fn();
  const owner = address("11111111111111111111111111111111");
  class Keys {
    #identity = "original-key-holder";
    async prove() {
      return this.#identity;
    }
  }
  const keys = new Keys();
  const ctx = {
    owner,
    keys,
    assertActive: () => {},
    signal: new AbortController().signal,
    wallet: { balance: () => ({ amount: 0n }) },
    client: {},
    submit: vi.fn(async (_tx, onSending) => {
      expect(report.mock.calls.at(-1)?.[0]).toBe("signing");
      onSending();
      return { signature: "confirmed", slot: 42n };
    }),
  } as unknown as PrivateWalletContext;
  vi.mocked(buildTransferTransaction).mockImplementation(
    async (input, request) => {
      expect(report.mock.calls.map(([stage]) => stage)).toEqual(["preparing"]);
      expect(await input.keys.prove({} as never, request)).toBe(
        "original-key-holder"
      );
      expect(report.mock.calls.at(-1)?.[0]).toBe("proving");
      return {} as never;
    }
  );
  vi.mocked(syncWallet).mockImplementation(async () => {
    expect(report.mock.calls.at(-1)).toEqual(["confirmed", "confirmed"]);
    return undefined as never;
  });
  await transferSol(ctx, owner, 1n, report);
  expect(report.mock.calls.map(([stage]) => stage)).toEqual([
    "preparing",
    "proving",
    "signing",
    "sending",
    "confirmed",
    "done",
  ]);
});

it("never submits or reports confirmation after a proof failure", async () => {
  const report = vi.fn();
  const owner = address("11111111111111111111111111111111");
  const ctx = {
    owner,
    keys: { prove: vi.fn().mockRejectedValue(new Error("Proof failed")) },
    assertActive: () => {},
    signal: new AbortController().signal,
    wallet: {},
    client: {},
    submit: vi.fn(),
  } as unknown as PrivateWalletContext;
  vi.mocked(buildTransferTransaction).mockImplementation(async (input) => {
    await input.keys.prove({} as never);
    return {} as never;
  });
  await expect(transferSol(ctx, owner, 1n, report)).rejects.toThrow(
    "Proof failed"
  );
  expect(ctx.submit).not.toHaveBeenCalled();
  expect(report.mock.calls.map(([stage]) => stage)).toEqual([
    "preparing",
    "proving",
  ]);
});

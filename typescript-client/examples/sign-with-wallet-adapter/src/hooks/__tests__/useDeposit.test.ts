import { beforeEach, describe, expect, it, vi } from "vitest";
import { address } from "@solana/kit";
import {
  buildDepositTransaction,
  buildTransferTransaction,
  buildWithdrawalTransaction,
  syncWallet,
} from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../usePrivateWallet";
import {
  BalanceSyncError,
  depositSol,
  transferSol,
  withdrawSol,
} from "../useDeposit";
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
        authority: {
          solanaPublicKey: () => owner,
          shieldedAddress: async () => ({}),
        },
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
        "Transaction confirmed",
      );
      expect(ctx.submit).toHaveBeenCalledTimes(1);
      expect(syncWallet).toHaveBeenCalledWith(
        expect.objectContaining({ config: { requireSlot: 42n } }),
      );
    },
  );
});

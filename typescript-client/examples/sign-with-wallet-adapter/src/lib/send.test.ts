import { beforeEach, describe, expect, it, vi } from "vitest";
import { signTransactionWithSigners } from "@solana/kit";
import type { createZolanaClient } from "@heliuslabs/zolana";
import { submitFactory } from "./send";
const mocks = vi.hoisted(() => ({ send: vi.fn() }));
vi.mock("@solana/kit", () => ({
  signTransactionWithSigners: vi.fn(),
  assertIsTransactionWithBlockhashLifetime: vi.fn(),
  getSignatureFromTransaction: () => "signature",
  sendAndConfirmTransactionFactory: () => mocks.send,
}));
beforeEach(() => vi.clearAllMocks());
describe("submission session guard", () => {
  it("does not broadcast when the session changes while signing", async () => {
    let active = true;
    vi.mocked(signTransactionWithSigners).mockImplementation(async () => {
      active = false;
      return {} as never;
    });
    const submit = submitFactory(
      {} as Awaited<ReturnType<typeof createZolanaClient>>,
      {} as never,
      () => {
        if (!active) throw new Error("Wallet changed");
      },
    );
    await expect(submit({} as never)).rejects.toThrow("Wallet changed");
    expect(mocks.send).not.toHaveBeenCalled();
  });
  it("keeps the existing two-argument submission API working", async () => {
    vi.mocked(signTransactionWithSigners).mockResolvedValue({} as never);
    const client = { confirmTransaction: vi.fn().mockResolvedValue(12n) };
    const submit = submitFactory(
      client as unknown as Awaited<ReturnType<typeof createZolanaClient>>,
      {} as never,
    );
    await expect(submit({} as never)).resolves.toEqual({
      signature: "signature",
      slot: 12n,
    });
    expect(mocks.send).toHaveBeenCalledTimes(1);
  });
});

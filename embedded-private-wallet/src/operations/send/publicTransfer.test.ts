import { beforeEach, expect, it, vi } from "vitest";
import {
  address,
  blockhash,
  getBase58Decoder,
  getCompiledTransactionMessageDecoder,
  signature,
} from "@solana/kit";
import {
  getTransferSolInstructionDataDecoder,
  SYSTEM_PROGRAM_ADDRESS,
} from "@solana-program/system";
import type { PublicWalletContext } from "../../lib/walletContext";
import { transferPublicSol } from "./publicTransfer";

const owner = address("So11111111111111111111111111111111111111112");
const recipient = address(
  getBase58Decoder().decode(new Uint8Array(32).fill(2)),
);
let ctx: PublicWalletContext;
let controller: AbortController;
let fee: ReturnType<typeof vi.fn>;
beforeEach(() => {
  controller = new AbortController();
  fee = vi.fn(async () => ({ value: 5_000n }));
  ctx = {
    owner,
    signal: controller.signal,
    assertActive: () => controller.signal.throwIfAborted(),
    client: {
      getLatestBlockhash: vi.fn(async () => ({
        blockhash: blockhash(owner),
        lastValidBlockHeight: 100n,
      })),
      getBalance: vi.fn(async () => 10_000_000n),
      solanaRpc: { getFeeForMessage: vi.fn(() => ({ send: fee })) },
    },
    submit: vi.fn(async (_tx, onSending) => {
      onSending?.();
      return { signature: signature("1".repeat(64)), slot: 100n };
    }),
  } as unknown as PublicWalletContext;
});
it("compiles a standard SOL instruction with the exact source, recipient and lamports", async () => {
  const progress = vi.fn();
  await transferPublicSol(ctx, recipient, 1_234_567n, progress);
  const transaction = vi.mocked(ctx.submit).mock.calls[0][0];
  const message = getCompiledTransactionMessageDecoder().decode(
    transaction.messageBytes,
  );
  if (message.version !== 0) throw new Error("Expected v0 transaction");
  expect(message.staticAccounts[0]).toBe(owner);
  expect(message.instructions).toHaveLength(1);
  const instruction = message.instructions[0];
  expect(message.staticAccounts[instruction.programAddressIndex]).toBe(
    SYSTEM_PROGRAM_ADDRESS,
  );
  expect(
    instruction.accountIndices?.map((index) => message.staticAccounts[index]),
  ).toEqual([owner, recipient]);
  expect(
    getTransferSolInstructionDataDecoder().decode(instruction.data!),
  ).toMatchObject({ amount: 1_234_567n });
  expect(ctx.client.getBalance).toHaveBeenCalledWith(owner, {
    signal: controller.signal,
  });
  expect(progress.mock.calls.map((call) => call[0])).toEqual([
    "preparing",
    "signing",
    "sending",
    "confirmed",
  ]);
});
it("allows the amount plus the exact fee", async () => {
  await transferPublicSol(ctx, recipient, 9_995_000n);
  expect(ctx.submit).toHaveBeenCalledTimes(1);
});
it("rejects insufficient fee funds before signing", async () => {
  await expect(transferPublicSol(ctx, recipient, 10_000_000n)).rejects.toThrow(
    "Leave 0.000005 SOL",
  );
  expect(ctx.submit).not.toHaveBeenCalled();
});
it.each([0n, -1n, 2n ** 64n])(
  "rejects invalid amount %s before any RPC or signature",
  async (amount) => {
    await expect(transferPublicSol(ctx, recipient, amount)).rejects.toThrow();
    expect(ctx.client.getLatestBlockhash).not.toHaveBeenCalled();
    expect(ctx.submit).not.toHaveBeenCalled();
  },
);
it("rejects invalid recipients before building", async () => {
  await expect(
    transferPublicSol(ctx, "invalid" as never, 1n),
  ).rejects.toThrow();
  expect(ctx.client.getLatestBlockhash).not.toHaveBeenCalled();
});
it("requires a valid fee estimate", async () => {
  fee.mockResolvedValue({ value: null });
  await expect(transferPublicSol(ctx, recipient, 1n)).rejects.toThrow(
    "estimate",
  );
  expect(ctx.submit).not.toHaveBeenCalled();
});
it("does not sign after an account change during the fee request", async () => {
  fee.mockImplementation(async () => {
    controller.abort();
    return { value: 5_000n };
  });
  await expect(transferPublicSol(ctx, recipient, 1n)).rejects.toThrow();
  expect(ctx.submit).not.toHaveBeenCalled();
});
it("propagates signing failure without reporting confirmation", async () => {
  vi.mocked(ctx.submit).mockRejectedValue(
    new Error("InvalidTransactionSignature"),
  );
  const progress = vi.fn();
  await expect(transferPublicSol(ctx, recipient, 1n, progress)).rejects.toThrow(
    "InvalidTransactionSignature",
  );
  expect(progress.mock.calls.map((call) => call[0])).not.toContain("confirmed");
});

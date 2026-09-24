import { beforeAll, beforeEach, expect, it, vi } from "vitest";
import { initializePoseidon } from "@heliuslabs/zolana";
import {
  getDepositInstructionAsync,
  getTransactInstruction,
} from "@heliuslabs/zolana/instructions";
import { resolveRegisteredAddress } from "@heliuslabs/zolana/wallet";
import {
  AssetRegistry,
  ConfidentialTransfer,
  WithdrawalTarget,
  type SppProofInputs,
} from "@heliuslabs/zolana/transaction";
import { getCompiledTransactionMessageDecoder } from "@solana/kit";
import { readPrivateState } from "../../lib/readPrivateState";
import {
  bytes,
  depositEvent,
  fixtureContext,
  instructionData,
  keypair,
} from "../../lib/__tests__/fixtures";
import { depositSol } from "./deposit";
import { transferSol } from "./transfer";
import { withdrawSol } from "./withdraw";
import { selectSolInputs, proveSpend } from "../../lib/privateSpend";
vi.mock("../../lib/readPrivateState", () => ({ readPrivateState: vi.fn() }));
vi.mock("@heliuslabs/zolana/wallet", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@heliuslabs/zolana/wallet")>()),
  resolveRegisteredAddress: vi.fn(),
}));
vi.mock("@heliuslabs/zolana/instructions", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@heliuslabs/zolana/instructions")>();
  return {
    ...actual,
    getDepositInstructionAsync: vi.fn(actual.getDepositInstructionAsync),
    getTransactInstruction: vi.fn(actual.getTransactInstruction),
  };
});
beforeAll(initializePoseidon);
let fixture: ReturnType<typeof fixtureContext>;
let recipient: ReturnType<typeof keypair>;
let proofInputs: SppProofInputs;
beforeEach(() => {
  vi.clearAllMocks();
  fixture = fixtureContext();
  recipient = keypair(8);
  const { ctx, key } = fixture;
  const note = depositEvent(key, 10_000_000n).note;
  vi.mocked(readPrivateState)
    .mockReset()
    .mockResolvedValue({
      notes: [note],
      balances: [],
      history: [],
      registry: new AssetRegistry(),
    });
  vi.mocked(resolveRegisteredAddress).mockReset().mockResolvedValue({
    owner: recipient.shieldedAddress().solanaAddress(),
    address: recipient.shieldedAddress(),
    viewTag: recipient.shieldedAddress().confidentialViewTag(),
  });
  vi.spyOn(ctx.keys, "prove").mockResolvedValue({} as never);
  ctx.client.proveTransact = vi.fn(
    async (input, authority, _config, request) => {
      proofInputs = input;
      await authority.prove({} as never, request);
      return instructionData(input) as never;
    },
  );
});

it("builds a real SOL deposit instruction with the chosen amount and owner", async () => {
  const { ctx } = fixture;
  await depositSol(ctx, 1_234_567n);
  expect(getDepositInstructionAsync).toHaveBeenCalledWith(
    expect.objectContaining({
      depositor: ctx.owner,
      deposits: [
        expect.objectContaining({
          amount: 1_234_567n,
          recipientOwnerHash: ctx.keys.address().ownerHash(),
          asset: { kind: "sol" },
        }),
      ],
    }),
  );
  const transaction = vi.mocked(ctx.submit).mock.calls[0][0];
  const decoded = getCompiledTransactionMessageDecoder().decode(
    transaction.messageBytes,
  );
  expect(decoded.staticAccounts[0]).toBe(ctx.owner);
  expect(decoded.version).toBe(0);
  if (decoded.version !== 0) throw new Error("Expected v0 transaction");
  expect(decoded.instructions).toHaveLength(1);
});

it.each(["transfer", "withdraw"])(
  "builds a real %s instruction with exact settlement and change",
  async (action) => {
    const { ctx } = fixture;
    const amount = 1_234_567n;
    if (action === "transfer")
      await transferSol(
        ctx,
        recipient.shieldedAddress().solanaAddress(),
        amount,
      );
    else await withdrawSol(ctx, amount);
    const actual = vi.mocked(getTransactInstruction).mock.calls[0][0];
    expect(actual.payer).toBe(ctx.owner);
    const change = proofInputs.outputs
      .slice(0, 2)
      .reduce((sum, o) => sum + o.amount, 0n);
    expect(change).toBe(10_000_000n - amount);
    if (action === "transfer") {
      expect(actual.withdrawal).toBeUndefined();
      expect(actual.data.interfaceTransfers).toEqual([]);
      expect(proofInputs.outputs[2].ownerAddress?.toBytes()).toEqual(
        recipient.shieldedAddress().toBytes(),
      );
      expect(proofInputs.outputs[2].amount).toBe(amount);
    } else {
      expect(actual.withdrawal).toEqual({ kind: "sol", recipient: ctx.owner });
      expect(actual.data.interfaceTransfers).toEqual([
        { kind: "solWithdrawal", amount },
      ]);
    }
    expect(readPrivateState).toHaveBeenCalledTimes(2);
    expect(readPrivateState).toHaveBeenLastCalledWith(ctx, {
      requireSlot: 99n,
    });
    const tx = vi.mocked(ctx.submit).mock.calls[0][0];
    const decoded = getCompiledTransactionMessageDecoder().decode(
      tx.messageBytes,
    );
    if (decoded.version !== 0) throw new Error("Expected v0 transaction");
    expect(decoded.instructions).toHaveLength(2);
  },
);

it.each(["deposit", "transfer", "withdraw"])(
  "retains the %s receipt if the post-confirmation read fails",
  async (action) => {
    const { ctx } = fixture;
    if (action === "deposit")
      vi.mocked(readPrivateState).mockRejectedValueOnce(new Error("offline"));
    else
      vi.mocked(readPrivateState)
        .mockResolvedValueOnce({
          notes: [depositEvent(fixture.key, 10_000_000n).note],
          balances: [],
          history: [],
          registry: new AssetRegistry(),
        })
        .mockRejectedValueOnce(new Error("offline"));
    const operation =
      action === "deposit"
        ? depositSol(ctx)
        : action === "transfer"
          ? transferSol(ctx, recipient.shieldedAddress().solanaAddress())
          : withdrawSol(ctx);
    await expect(operation).rejects.toMatchObject({
      name: "BalanceSyncError",
      signature: expect.any(String),
    });
    expect(ctx.submit).toHaveBeenCalledTimes(1);
  },
);

it("announces real proving, signing, sending, and confirmation before the final read", async () => {
  const { ctx } = fixture;
  const report = vi.fn();
  const submit = vi.mocked(ctx.submit).getMockImplementation()!;
  vi.mocked(ctx.submit).mockImplementation(async (tx, onSending) => {
    expect(report.mock.calls.at(-1)?.[0]).toBe("signing");
    onSending?.();
    return submit(tx);
  });
  const original = vi.mocked(readPrivateState).getMockImplementation()!;
  vi.mocked(readPrivateState).mockImplementation(async (input, options) => {
    if (options?.requireSlot)
      expect(report.mock.calls.at(-1)?.[0]).toBe("confirmed");
    return original(input, options);
  });
  await transferSol(
    ctx,
    recipient.shieldedAddress().solanaAddress(),
    1n,
    report,
  );
  expect(report.mock.calls.map(([stage]) => stage)).toEqual([
    "preparing",
    "proving",
    "signing",
    "sending",
    "confirmed",
    "done",
  ]);
});

it("destroys transaction keys even if the session changes while TVC responds", async () => {
  const { ctx, controller } = fixture;
  const original = ctx.keys.transactionKeys.bind(ctx.keys);
  let destroy = vi.fn();
  vi.spyOn(ctx.keys, "transactionKeys").mockImplementation(async (...args) => {
    const keys = await original(...args);
    destroy = vi.spyOn(keys[0], "destroy");
    controller.abort(new Error("Wallet changed"));
    return keys;
  });
  await expect(withdrawSol(ctx, 1n)).rejects.toThrow("Wallet changed");
  expect(destroy).toHaveBeenCalledOnce();
  expect(ctx.submit).not.toHaveBeenCalled();
});

it("rejects altered proof data and service failure before signing", async () => {
  const { ctx } = fixture;
  vi.mocked(ctx.client.proveTransact).mockImplementationOnce(
    async (input) =>
      ({ ...instructionData(input), privateTxHash: bytes(4) }) as never,
  );
  await expect(withdrawSol(ctx, 1n)).rejects.toThrow("does not match");
  vi.mocked(ctx.client.proveTransact).mockRejectedValueOnce(
    new Error("Proof failed"),
  );
  await expect(withdrawSol(ctx, 1n)).rejects.toThrow("Proof failed");
  expect(ctx.submit).not.toHaveBeenCalled();
});

it("rejects invalid recipients, unregistered recipients, and invalid amounts before proving", async () => {
  const { ctx } = fixture;
  await expect(transferSol(ctx, "invalid" as never)).rejects.toThrow();
  expect(resolveRegisteredAddress).not.toHaveBeenCalled();
  vi.mocked(resolveRegisteredAddress).mockResolvedValueOnce(undefined);
  await expect(
    transferSol(ctx, recipient.shieldedAddress().solanaAddress()),
  ).rejects.toThrow("not registered");
  await expect(depositSol(ctx, 0n)).rejects.toThrow("greater than zero");
  await expect(withdrawSol(ctx, 0n)).rejects.toThrow("greater than zero");
  expect(ctx.client.proveTransact).not.toHaveBeenCalled();
  expect(ctx.submit).not.toHaveBeenCalled();
});

it("excludes spent and ring notes, rejects multiple trees and unsupported input counts", () => {
  const { ctx, key } = fixture;
  const notes = Array.from(
    { length: 6 },
    (_, i) => depositEvent(key, 1n, i + 1).note,
  );
  expect(() => selectSolInputs(ctx, notes, 6n)).toThrow("too many notes");
  expect(() =>
    selectSolInputs(ctx, [{ ...notes[0], spent: true }], 1n),
  ).toThrow("spendable");
  expect(() =>
    selectSolInputs(ctx, [{ ...notes[0], ringDataHash: bytes(1) }], 1n),
  ).toThrow("spendable");
  expect(() =>
    selectSolInputs(
      ctx,
      [
        notes[0],
        {
          ...notes[1],
          outputContext: {
            ...notes[1].outputContext,
            tree: recipient.shieldedAddress().solanaAddress(),
          },
        },
      ],
      1n,
    ),
  ).toThrow("multiple trees");
});

it("deposits to another registered private wallet while keeping the connected wallet as payer", async () => {
  const { ctx } = fixture;
  const destination = recipient.shieldedAddress();
  await depositSol(ctx, 1_234_567n, destination.solanaAddress());
  expect(resolveRegisteredAddress).toHaveBeenCalledWith(
    { rpc: ctx.client, owner: destination.solanaAddress() },
    { signal: ctx.signal },
  );
  expect(getDepositInstructionAsync).toHaveBeenCalledWith(
    expect.objectContaining({
      depositor: ctx.owner,
      deposits: [
        expect.objectContaining({
          amount: 1_234_567n,
          recipientOwnerHash: destination.ownerHash(),
          viewTag: destination.confidentialViewTag(),
        }),
      ],
    }),
  );
});
it("does not look up another recipient for a deposit to self", async () => {
  await depositSol(fixture.ctx, 1n);
  expect(resolveRegisteredAddress).not.toHaveBeenCalled();
});
it("rejects missing or mismatched deposit registration before building", async () => {
  const destination = recipient.shieldedAddress().solanaAddress();
  vi.mocked(resolveRegisteredAddress).mockResolvedValueOnce(undefined);
  await expect(depositSol(fixture.ctx, 1n, destination)).rejects.toThrow(
    "not registered",
  );
  await expect(
    depositSol(fixture.ctx, 1n, keypair(9).shieldedAddress().solanaAddress()),
  ).rejects.toThrow("does not match");
  expect(getDepositInstructionAsync).not.toHaveBeenCalled();
  expect(fixture.ctx.submit).not.toHaveBeenCalled();
});
it("withdraws to another public address and keeps private change with the sender", async () => {
  const { ctx } = fixture;
  const destination = recipient.shieldedAddress().solanaAddress();
  await withdrawSol(ctx, 1_234_567n, destination);
  const actual = vi.mocked(getTransactInstruction).mock.calls[0][0];
  expect(actual.withdrawal).toEqual({ kind: "sol", recipient: destination });
  expect(actual.payer).toBe(ctx.owner);
  expect(proofInputs.externalData.interfaceTransfers[0]).toMatchObject({
    userSolAccount: destination,
    amount: 1_234_567n,
    isDeposit: false,
  });
  const change = proofInputs.outputs.filter((output) => !output.isDummy());
  expect(change.reduce((sum, output) => sum + output.amount, 0n)).toBe(
    8_765_433n,
  );
  expect(
    change.every(
      (output) => output.ownerAddress?.solanaAddress() === ctx.owner,
    ),
  ).toBe(true);
  expect(resolveRegisteredAddress).not.toHaveBeenCalled();
});
it.each(["deposit", "withdraw"])(
  "rejects invalid %s destinations before reads or builders",
  async (action) => {
    const operation = action === "deposit" ? depositSol : withdrawSol;
    await expect(
      operation(fixture.ctx, 1n, "invalid" as never),
    ).rejects.toThrow();
    expect(readPrivateState).not.toHaveBeenCalled();
    expect(getDepositInstructionAsync).not.toHaveBeenCalled();
    expect(fixture.ctx.submit).not.toHaveBeenCalled();
  },
);
it("discards a deposit registry response after an account change", async () => {
  vi.mocked(resolveRegisteredAddress).mockImplementationOnce(async () => {
    fixture.controller.abort(new Error("Wallet changed"));
    return {
      owner: recipient.shieldedAddress().solanaAddress(),
      address: recipient.shieldedAddress(),
      viewTag: recipient.shieldedAddress().confidentialViewTag(),
    };
  });
  await expect(
    depositSol(fixture.ctx, 1n, recipient.shieldedAddress().solanaAddress()),
  ).rejects.toThrow("Wallet changed");
  expect(getDepositInstructionAsync).not.toHaveBeenCalled();
});

it("rejects a prepared withdrawal addressed to anyone other than the chosen recipient", async () => {
  const { ctx, key } = fixture;
  const notes = [depositEvent(key, 10_000_000n).note];
  const transfer = new ConfidentialTransfer(
    ctx.keys.address(),
    selectSolInputs(ctx, notes, 1n),
    ctx.owner,
  );
  const { SOL_MINT } = await import("@heliuslabs/zolana");
  transfer.withdraw(
    SOL_MINT,
    1n,
    WithdrawalTarget.sol({ recipient: ctx.owner }),
  );
  const transactionKeys = vi.spyOn(ctx.keys, "transactionKeys");
  await expect(
    proveSpend(ctx, transfer.prepare(), new AssetRegistry(), {
      amount: 1n,
      withdrawalRecipient: recipient.shieldedAddress().solanaAddress(),
    }),
  ).rejects.toThrow("does not match");
  expect(transactionKeys).not.toHaveBeenCalled();
  expect(ctx.client.proveTransact).not.toHaveBeenCalled();
});

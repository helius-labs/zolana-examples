import { beforeAll, expect, it, vi } from "vitest";
import {
  initializePoseidon,
  syncWallet,
  Wallet,
  SOL_MINT,
} from "@heliuslabs/zolana";
import { LocalKeys } from "@heliuslabs/zolana/client";
import { backfillAssetRegistry } from "@heliuslabs/zolana/wallet";
import { ViewingKey } from "@heliuslabs/zolana/keypair";
import { readPrivateState } from "./readPrivateState";
import {
  bytes,
  depositEvent,
  fixtureContext,
  indexContext,
  keypair,
  spendEvent,
} from "./__tests__/fixtures";

vi.mock("@heliuslabs/zolana/wallet", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@heliuslabs/zolana/wallet")>();
  return {
    ...actual,
    backfillAssetRegistry: vi.fn(actual.backfillAssetRegistry),
  };
});
beforeAll(initializePoseidon);

it("matches SDK sync for deposits, encrypted transfers, spent notes, and history", async () => {
  const { ctx, client, key } = fixtureContext();
  const deposit = depositEvent(key, 10_000_000n);
  const spentElsewhere = spendEvent(key, deposit.note, keypair(8), 3_000_000n);
  vi.mocked(client.getEncryptedUtxosByTags).mockResolvedValue({
    context: indexContext,
    matches: [deposit.match],
  } as never);
  vi.mocked(client.getShieldedTransactionsByNullifiers).mockResolvedValue({
    context: indexContext,
    transactions: [spentElsewhere],
  } as never);
  const fresh = await readPrivateState(ctx);
  const reference = new Wallet({ identity: ctx.keys.address() });
  await syncWallet({ client: ctx.client, keys: ctx.keys, wallet: reference });
  expect(fresh.balances).toEqual(reference.balances(true));
  expect(fresh.history).toEqual(reference.privateTransactions());
  expect(fresh.notes).toEqual(reference.utxos().filter((n) => !n.spent));
  expect(fresh.balances.find((b) => b.mint === SOL_MINT)?.amount).toBe(
    7_000_000n,
  );
  expect(fresh.history.length).toBeGreaterThan(1);
});

it("continues nullifier discovery across chained spends made on another device", async () => {
  const { ctx, client, key } = fixtureContext();
  const deposit = depositEvent(key, 10_000_000n);
  const first = spendEvent(key, deposit.note, keypair(8), 3_000_000n);
  client.getEncryptedUtxosByTags.mockResolvedValue({
    context: indexContext,
    matches: [deposit.match],
  } as never);
  client.getShieldedTransactionsByNullifiers.mockResolvedValue({
    context: indexContext,
    transactions: [first],
  } as never);
  const intermediate = await readPrivateState(ctx);
  const second = spendEvent(
    key,
    intermediate.notes.find((n) => n.utxo.amount === 7_000_000n)!,
    keypair(9),
    2_000_000n,
    40,
  );
  client.getShieldedTransactionsByNullifiers
    .mockReset()
    .mockResolvedValueOnce({
      context: indexContext,
      transactions: [first],
    } as never)
    .mockResolvedValue({
      context: indexContext,
      transactions: [second],
    } as never);
  const final = await readPrivateState(ctx);
  expect(final.balances[0].amount).toBe(5_000_000n);
  expect(client.getShieldedTransactionsByNullifiers).toHaveBeenCalledTimes(3);
});

it("reads all pages, deduplicates overlapping outputs, and preserves same-signature deposits", async () => {
  const { ctx, client, key } = fixtureContext();
  const a = depositEvent(key, 1n, 1),
    b = depositEvent(key, 2n, 2);
  b.match.txSignature = a.match.txSignature;
  client.getEncryptedUtxosByTags
    .mockResolvedValueOnce({
      context: indexContext,
      matches: [a.match],
      nextCursor: new Uint8Array([1]),
    } as never)
    .mockResolvedValueOnce({
      context: indexContext,
      matches: [a.match, b.match],
    } as never);
  const result = await readPrivateState(ctx, { requireSlot: 42n });
  expect(result.balances[0].amount).toBe(3n);
  expect(result.history).toHaveLength(2);
  expect(client.getEncryptedUtxosByTags.mock.calls[1]).toBeDefined();
  expect(client.getShieldedTransactionsByTags).toHaveBeenCalledWith(
    expect.objectContaining({ limit: 1_000 }),
    expect.objectContaining({ requireSlot: 42n }),
    expect.anything(),
  );
  expect(client.getEncryptedUtxosByTags).toHaveBeenCalledWith(
    expect.objectContaining({ cursor: new Uint8Array([1]) }),
    expect.not.objectContaining({ requireSlot: 42n }),
    expect.anything(),
  );
});

it("rejects cursor loops and failed pages without returning partial balances", async () => {
  const { ctx, client, key } = fixtureContext();
  client.getEncryptedUtxosByTags.mockResolvedValue({
    context: indexContext,
    matches: [depositEvent(key, 1n).match],
    nextCursor: new Uint8Array([1]),
  } as never);
  await expect(readPrivateState(ctx)).rejects.toThrow("cursor");
  client.getEncryptedUtxosByTags
    .mockReset()
    .mockResolvedValueOnce({
      context: indexContext,
      matches: [],
      nextCursor: new Uint8Array([2]),
    } as never)
    .mockRejectedValueOnce(new Error("offline"));
  await expect(readPrivateState(ctx)).rejects.toThrow("offline");
});

it("starts every read from the beginning without retaining a previous balance", async () => {
  const { ctx, client, key } = fixtureContext();
  client.getEncryptedUtxosByTags.mockResolvedValueOnce({
    context: indexContext,
    matches: [depositEvent(key, 1n).match],
  } as never);
  expect((await readPrivateState(ctx)).balances[0].amount).toBe(1n);
  expect((await readPrivateState(ctx)).balances).toEqual([]);
});

it("rejects a different owner or viewing identity before publishing query tags", async () => {
  const { ctx, client } = fixtureContext();
  await expect(readPrivateState({ ...ctx, owner: SOL_MINT })).rejects.toThrow(
    "identity",
  );
  const wrong = { ...ctx, keys: Object.create(ctx.keys) };
  // Do not lose private-field binding when wrapping a key holder.
  wrong.keys = {
    address: ctx.keys.address.bind(ctx.keys),
    viewingPublicKeys: () => [],
    decrypt: ctx.keys.decrypt.bind(ctx.keys),
    derive: ctx.keys.derive.bind(ctx.keys),
    transactionKeys: ctx.keys.transactionKeys.bind(ctx.keys),
    prove: ctx.keys.prove.bind(ctx.keys),
    proveMerge: ctx.keys.proveMerge.bind(ctx.keys),
  };
  await expect(readPrivateState(wrong)).rejects.toThrow("viewing keys");
  expect(client.getShieldedTransactionsByTags).not.toHaveBeenCalled();
});

it("includes held historical viewing keys and chunks query tags", async () => {
  const { ctx, client, key } = fixtureContext();
  const extra = Array.from({ length: 64 }, (_, i) =>
    ViewingKey.fromBytes(bytes(i + 1)),
  );
  const keys = LocalKeys.fromKeys(
    {
      address: key.shieldedAddress(),
      viewingKeys: [key.viewingKey(), ...extra],
      nullifierKey: key.nullifierKey(),
    },
    { prove: vi.fn(), proveMerge: vi.fn() },
  );
  await readPrivateState({ ...ctx, keys });
  expect(client.getShieldedTransactionsByTags).toHaveBeenCalledTimes(2);
  expect(client.getShieldedTransactionsByTags).toHaveBeenCalledWith(
    expect.objectContaining({
      tags: expect.arrayContaining([extra[0].publicKey().x()]),
    }),
    expect.anything(),
    expect.anything(),
  );
  keys.destroy();
  extra.forEach((k) => k.destroy());
});

it("propagates session and caller cancellation through indexer and TVC calls", async () => {
  const { ctx, client, controller } = fixtureContext();
  const caller = new AbortController();
  client.getShieldedTransactionsByTags.mockImplementation(
    async (...args: unknown[]) => {
      controller.abort(new Error("Wallet changed"));
      expect((args[2] as { signal: AbortSignal }).signal.aborted).toBe(true);
      return { context: indexContext, transactions: [] };
    },
  );
  await expect(
    readPrivateState(ctx, { signal: caller.signal }),
  ).rejects.toThrow("Wallet changed");
  expect(client.getEncryptedUtxosByTags).not.toHaveBeenCalled();
});

it("resolves indexed assets through the SDK and rejects unresolved balances", async () => {
  const { ctx, client, key } = fixtureContext();
  const mint = keypair(12).shieldedAddress().solanaAddress();
  client.getEncryptedUtxosByTags.mockResolvedValue({
    context: indexContext,
    matches: [depositEvent(key, 5n, 1, mint).match],
  } as never);
  vi.mocked(backfillAssetRegistry).mockImplementationOnce(async (wallet) => {
    wallet.ensureAsset(2n, mint);
    return 1;
  });
  expect((await readPrivateState(ctx)).balances[0]).toMatchObject({
    mint,
    amount: 5n,
  });
  vi.mocked(backfillAssetRegistry).mockResolvedValueOnce(0);
  await expect(readPrivateState(ctx)).rejects.toThrow("could not be resolved");
});

it("fails a malformed indexed transaction instead of presenting partial history", async () => {
  const { ctx, client, key } = fixtureContext();
  const deposit = depositEvent(key, 10n);
  const broken = spendEvent(key, deposit.note, keypair(8), 1n);
  const outputSlots = broken.outputSlots.map((slot) => ({
    ...slot,
    payload: new Uint8Array([255]),
  }));
  client.getShieldedTransactionsByTags.mockResolvedValue({
    context: indexContext,
    transactions: [{ ...broken, outputSlots }],
  } as never);
  await expect(readPrivateState(ctx)).rejects.toThrow("could not be parsed");
});

it("propagates TVC decryption failure and passes the cancellation signal to the holder", async () => {
  const { ctx, client, key } = fixtureContext();
  const sender = keypair(8);
  const transfer = spendEvent(sender, depositEvent(sender, 10n).note, key, 5n);
  client.getShieldedTransactionsByTags.mockResolvedValue({
    context: indexContext,
    transactions: [transfer],
  } as never);
  const decrypt = vi
    .spyOn(ctx.keys, "decrypt")
    .mockRejectedValueOnce(new Error("TVC unavailable"));
  await expect(readPrivateState(ctx)).rejects.toThrow();
  expect(decrypt).toHaveBeenCalledWith(expect.any(Array), {
    signal: ctx.signal,
  });
});

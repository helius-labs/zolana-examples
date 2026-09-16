import { Wallet, initializePoseidon } from "@heliuslabs/zolana";
import { backfillAssetRegistry } from "@heliuslabs/zolana/wallet";
import {
  checkKeysIdentity,
  decodeOutputData,
  decryptTransactions,
  type IndexedShieldedTransaction,
} from "@heliuslabs/zolana/transaction";
import {
  DEFAULT_INDEXER_POLL_CONFIG,
  type IndexerRpcConfig,
} from "@heliuslabs/zolana/client";
import { getAddressEncoder } from "@solana/kit";
import type { PrivateWalletContext } from "./walletContext";

export type ReadOptions = { requireSlot?: bigint; signal?: AbortSignal };
type ReadContext = Pick<
  PrivateWalletContext,
  "client" | "keys" | "owner" | "signal"
>;
const hex = (bytes: ArrayLike<number>) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const chunks = <T>(items: readonly T[]) =>
  Array.from({ length: Math.ceil(items.length / 64) }, (_, i) =>
    items.slice(i * 64, i * 64 + 64),
  );
const compare = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
const eventKey = (tx: IndexedShieldedTransaction) =>
  [
    tx.txSignature,
    ...tx.outputSlots.map(
      (s) =>
        `${s.outputContext.tree}:${s.outputContext.leafIndex}:${hex(s.outputContext.hash)}`,
    ),
    ...tx.nullifiers.map(hex),
  ].join("|");
const order = (a: IndexedShieldedTransaction, b: IndexedShieldedTransaction) =>
  a.slot < b.slot
    ? -1
    : a.slot > b.slot
      ? 1
      : compare(a.txSignature, b.txSignature) ||
        compare(eventKey(a), eventKey(b));

/** Full read: no retained wallet, cursors, notes, or history between calls. */
export async function readPrivateState(
  ctx: ReadContext,
  options: ReadOptions = {},
) {
  const signal = options.signal
    ? AbortSignal.any([ctx.signal, options.signal])
    : ctx.signal;
  const request = { signal };
  const active = () => signal.throwIfAborted();
  active();
  const identity = ctx.keys.address();
  if (identity.solanaAddress() !== ctx.owner)
    throw new Error(
      "Private wallet identity does not match the connected account.",
    );
  checkKeysIdentity(ctx.keys, identity);
  const viewingKeys = ctx.keys.viewingPublicKeys();
  if (
    !viewingKeys.length ||
    hex(viewingKeys[0].toBytes()) !== hex(identity.viewingPublicKey.toBytes())
  )
    throw new Error("Private wallet viewing keys do not match its identity.");
  await initializePoseidon();
  active();
  const tags = [
    ...new Map(
      [
        identity.signingPublicKey.confidentialViewTag(),
        ...viewingKeys.map((k) => k.x()),
      ].map((t) => [hex(t), t]),
    ).values(),
  ];
  // Gate the first read on confirmation. Later pages reuse that freshness floor.
  let requireSlot = options.requireSlot;
  const rpcConfig = (): IndexerRpcConfig => {
    const config = {
      poll: DEFAULT_INDEXER_POLL_CONFIG,
      ...(requireSlot === undefined ? {} : { requireSlot }),
    };
    requireSlot = undefined;
    return config;
  };
  const transactions = new Map<string, IndexedShieldedTransaction>();
  const deposits = new Map<string, IndexedShieldedTransaction>();
  async function pages<T extends { nextCursor?: Uint8Array }>(
    fetch: (cursor?: Uint8Array) => Promise<T>,
    consume: (page: T) => void,
  ) {
    let cursor: Uint8Array | undefined;
    const seen = new Set<string>();
    do {
      active();
      const page = await fetch(cursor);
      active();
      consume(page);
      cursor = page.nextCursor;
      if (cursor !== undefined) {
        if (!cursor.length || seen.has(hex(cursor)))
          throw new Error("Indexer returned a repeated or invalid cursor.");
        seen.add(hex(cursor));
      }
    } while (cursor !== undefined);
  }
  const collect = (rows: readonly IndexedShieldedTransaction[]) => {
    for (const tx of rows)
      if (!tx.proofless) transactions.set(eventKey(tx), tx);
  };
  for (const chunk of chunks(tags)) {
    await pages(
      (cursor) =>
        ctx.client.getShieldedTransactionsByTags(
          { tags: chunk, limit: 1_000, cursor },
          rpcConfig(),
          request,
        ),
      (page) => collect(page.transactions),
    );
    await pages(
      (cursor) =>
        ctx.client.getEncryptedUtxosByTags(
          { tags: chunk, limit: 1_000, cursor },
          rpcConfig(),
          request,
        ),
      (page) => {
        for (const match of page.matches) {
          if (match.txViewingPk !== undefined || match.salt !== undefined)
            continue;
          // Tag queries may include unrelated formats. The SDK handles commitment validation.
          try {
            decodeOutputData(match.outputSlot.payload);
          } catch {
            continue;
          }
          const tx: IndexedShieldedTransaction = {
            slot: match.slot,
            txSignature: match.txSignature,
            outputSlots: [match.outputSlot],
            messages: [],
            nullifiers: [],
            proofless: true,
          };
          deposits.set(eventKey(tx), tx);
        }
      },
    );
  }
  const wallet = new Wallet({ identity });
  const queried = new Set<string>();
  let registryLoaded = false;
  const addressBytes = getAddressEncoder();
  const ordered = () => [
    ...[...transactions.values()].sort(order),
    ...[...deposits.values()].sort((a, b) => {
      const left = a.outputSlots[0].outputContext,
        right = b.outputSlots[0].outputContext;
      return (
        compare(
          hex(addressBytes.encode(left.tree)),
          hex(addressBytes.encode(right.tree)),
        ) ||
        (left.leafIndex < right.leafIndex
          ? -1
          : left.leafIndex > right.leafIndex
            ? 1
            : order(a, b))
      );
    }),
  ];
  for (;;) {
    active();
    let report = await decryptTransactions({
      wallet,
      keys: ctx.keys,
      transactions: ordered(),
      context: request,
    });
    active();
    const unresolved = () =>
      report.unknownAssetIds.length > 0 ||
      report.unknownAssetFields.length > 0 ||
      wallet.utxos().some((entry) => {
        try {
          wallet.registry.assetId(entry.utxo.asset);
          return false;
        } catch {
          return true;
        }
      });
    if (unresolved() && !registryLoaded) {
      await backfillAssetRegistry(wallet, ctx.client, request);
      active();
      registryLoaded = true;
      report = await decryptTransactions({
        wallet,
        keys: ctx.keys,
        transactions: ordered(),
        context: request,
      });
      active();
    }
    if (unresolved())
      throw new Error(
        "Private balance is incomplete: an indexed asset could not be resolved.",
      );
    if (report.unparsedTransactions > 0)
      throw new Error(
        "Private history is incomplete: an indexed transaction could not be parsed.",
      );
    const nullifiers = [
      ...new Map(
        wallet
          .utxos()
          .filter((e) => !e.spent && !queried.has(hex(e.nullifier)))
          .map((e) => [hex(e.nullifier), e.nullifier]),
      ).values(),
    ];
    if (!nullifiers.length) break;
    const previousCount = transactions.size;
    for (const chunk of chunks(nullifiers)) {
      chunk.forEach((n) => queried.add(hex(n)));
      await pages(
        (cursor) =>
          ctx.client.getShieldedTransactionsByNullifiers(
            { nullifiers: chunk, limit: 1_000, cursor },
            rpcConfig(),
            request,
          ),
        (page) => collect(page.transactions),
      );
    }
    // An empty/duplicate spend response changes no notes; do not decrypt again.
    if (transactions.size === previousCount) break;
  }
  active();
  return {
    balances: wallet.balances(true),
    history: wallet.privateTransactions(),
    notes: wallet.utxos().filter((entry) => !entry.spent),
    registry: wallet.registry,
  };
}

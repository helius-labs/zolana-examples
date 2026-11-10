/**
 * The key record store (design.md): keeps every key record it receives. A
 * client never trusts it to say which record is current; it fetches the
 * records of a Settings account and picks the one that matches on-chain state.
 */
import type { Address } from "@solana/kit";

import {
  decodeKeyRecord,
  encodeKeyRecord,
  isCurrentKeyRecord,
  type KeyRecord,
  type OnChainKeyState,
} from "./keyRecord.js";

export interface KeyRecordStore {
  /** Stores an encoded key record. */
  put(record: KeyRecord): Promise<void>;
  /** Every stored record of `settings`, in any order. */
  list(settings: Address): Promise<readonly KeyRecord[]>;
}

/**
 * Keeps records in memory; for tests and single-process demos. A shared
 * deployment implements `KeyRecordStore` over its own storage service.
 */
export class InMemoryKeyRecordStore implements KeyRecordStore {
  readonly #records = new Map<Address, Uint8Array[]>();

  async put(record: KeyRecord): Promise<void> {
    const encoded = encodeKeyRecord(record);
    const existing = this.#records.get(record.settings) ?? [];
    existing.push(encoded);
    this.#records.set(record.settings, existing);
  }

  async list(settings: Address): Promise<readonly KeyRecord[]> {
    return (this.#records.get(settings) ?? [])
      .map(tryDecode)
      .filter((record) => record !== undefined);
  }
}

/** A store is untrusted: a record that does not decode is skipped. */
function tryDecode(data: Uint8Array): KeyRecord | undefined {
  try {
    return decodeKeyRecord(data);
  } catch {
    return undefined;
  }
}

/**
 * The current record of `state.settings`: the stored record that matches the
 * Squads signer set and the vault's registered viewing key.
 *
 * Once the vault is registered exactly one distinct record may match. The
 * epoch is not trusted then: anyone who can write to the store can copy the
 * current record under a higher epoch with other entries. Before
 * registration only the signer set is checked, so the highest epoch wins and
 * the store can swap the record (design.md, limitation 3).
 */
export async function currentKeyRecord(
  store: KeyRecordStore,
  state: OnChainKeyState,
): Promise<KeyRecord> {
  const matching = distinct(
    (await store.list(state.settings)).filter((record) =>
      isCurrentKeyRecord(record, state),
    ),
  );
  if (state.registeredViewingPublicKey !== undefined && matching.length > 1) {
    throw new Error(`several stored key records match ${state.settings}`);
  }
  const latest = matching.reduce<KeyRecord | undefined>(
    (best, record) =>
      best === undefined || record.epoch > best.epoch ? record : best,
    undefined,
  );
  if (latest === undefined) {
    throw new Error(
      `no stored key record matches the on-chain state of ${state.settings}`,
    );
  }
  if (matching.filter((record) => record.epoch === latest.epoch).length > 1) {
    throw new Error(`several stored key records match ${state.settings}`);
  }
  return latest;
}

/** Drops byte-identical copies, such as a record uploaded twice. */
function distinct(records: readonly KeyRecord[]): KeyRecord[] {
  const seen = new Set<string>();
  return records.filter((record) => {
    const key = Buffer.from(encodeKeyRecord(record)).toString("hex");
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

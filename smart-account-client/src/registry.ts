/**
 * The vault's user registry record, which holds the vault's current key
 * record (design.md, "Key Record"). `register` creates the record with the
 * shared keys; `set_data` then writes the key record into its data, inside
 * the same Squads transaction that registers or rotates the keys. Writing
 * needs the vault's signature, so only a threshold of signers can set a
 * record. `set_data` replaces the data, so earlier records survive only in
 * the transactions that set them.
 */
import {
  AccountRole,
  getBase58Encoder,
  type Address,
  type Instruction,
  type Signature,
} from "@solana/kit";
import type { SprClient } from "@heliuslabs/spr/client";
import { getUserRecordAddress } from "@heliuslabs/spr/addresses";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { USER_REGISTRY_PROGRAM_ID } from "@heliuslabs/spr/interface";
import type { Bytes33 } from "@heliuslabs/spr/keypair";
import { decodeUserRecordAccount } from "@heliuslabs/spr/wallet";

import { ByteReader, ByteWriter } from "./bytes.js";
import {
  decodeKeyRecord,
  encodeKeyRecord,
  isCurrentKeyRecord,
  type KeyRecord,
} from "./keyRecord.js";
import { SYSTEM_PROGRAM_ADDRESS } from "./squads.js";

const UPDATE_KEYS = 2;
const SET_DATA = 3;
/** The fixed user record header; the data follows it. */
const USER_RECORD_SIZE = 100;
const SIGNATURE_PAGE = 1000;
const base58 = getBase58Encoder();

/** What the vault's registry record says about its keys. */
export interface VaultRegistry {
  /** The registered shared viewing public key. */
  readonly viewingPublicKey: Bytes33;
  /** The key record in the record's data, if any. */
  readonly record: KeyRecord | undefined;
}

/** The vault's registry record, or undefined before Register. */
export async function fetchVaultRegistry(
  client: SprClient,
  vault: Address,
): Promise<VaultRegistry | undefined> {
  const { value } = await client.solanaRpc
    .getAccountInfo(await getUserRecordAddress(vault), {
      encoding: "base64",
      commitment: "confirmed",
    })
    .send();
  if (value === null) return undefined;
  const data = new Uint8Array(Buffer.from(value.data[0], "base64"));
  const header = decodeUserRecordAccount({
    owner: value.owner,
    data,
    lamports: value.lamports,
  });
  if (header.owner !== vault) {
    throw new Error(`the registry record of ${vault} names another owner`);
  }
  return {
    viewingPublicKey: header.viewingPublicKey,
    record: decodeRecordData(data.subarray(USER_RECORD_SIZE)),
  };
}

/**
 * The key record in a registry record's data: a u32 length, then exactly
 * one encoded record. No data means no record.
 */
export function decodeRecordData(data: Uint8Array): KeyRecord | undefined {
  if (data.length === 0) return undefined;
  const reader = new ByteReader(data, "registry data");
  const record = decodeKeyRecord(
    reader.bytes(reader.u32("data length"), "data"),
  );
  reader.end();
  return record;
}

/**
 * The current key record: the one in the vault's registry data. It must
 * belong to `settings`, name exactly the Squads signers, and hold the
 * registered viewing key; a Squads transaction that changed the signers
 * outside the rotate flow breaks the last two.
 */
export function currentKeyRecord(
  registry: VaultRegistry,
  settings: Address,
  signers: readonly Address[],
): KeyRecord {
  const record = registry.record;
  if (record === undefined) {
    throw new Error(`the vault of ${settings} has no key record`);
  }
  if (
    !isCurrentKeyRecord(record, {
      settings,
      signers,
      registeredViewingPublicKey: registry.viewingPublicKey,
    })
  ) {
    throw new Error(
      `the key record of ${settings} does not match its signers and registered key`,
    );
  }
  return record;
}

/**
 * Every key record a successful registry `set_data` wrote to `vault`'s
 * record, newest first, read from the record account's transaction history.
 * It includes the current record. Only the vault can sign `set_data`, so each
 * record went through a threshold of signers. A `set_data` whose data does
 * not decode as a key record is skipped.
 */
export async function fetchKeyRecordHistory(
  client: SprClient,
  vault: Address,
): Promise<KeyRecord[]> {
  const recordAddress = await getUserRecordAddress(vault);
  const records: KeyRecord[] = [];
  let before: Signature | undefined;
  for (;;) {
    const page = await client.solanaRpc
      .getSignaturesForAddress(recordAddress, {
        commitment: "confirmed",
        limit: SIGNATURE_PAGE,
        ...(before === undefined ? {} : { before }),
      })
      .send();
    for (const { signature, err } of page) {
      if (err !== null) continue;
      records.push(...(await setDataRecords(client, signature, recordAddress)));
    }
    const last = page.at(-1);
    if (page.length < SIGNATURE_PAGE || last === undefined) return records;
    before = last.signature;
  }
}

/** The key records the `set_data` instructions of one transaction wrote. */
async function setDataRecords(
  client: SprClient,
  signature: Signature,
  recordAddress: Address,
): Promise<KeyRecord[]> {
  const transaction = await client.solanaRpc
    .getTransaction(signature, {
      commitment: "confirmed",
      encoding: "json",
      maxSupportedTransactionVersion: 1,
    })
    .send();
  if (transaction === null || transaction.meta === null) return [];
  if (transaction.meta.err !== null) return [];
  const keys = [
    ...transaction.transaction.message.accountKeys,
    ...transaction.meta.loadedAddresses.writable,
    ...transaction.meta.loadedAddresses.readonly,
  ];
  // set_data runs as a Squads CPI, so it is an inner instruction.
  const instructions = [
    ...transaction.transaction.message.instructions,
    ...(transaction.meta.innerInstructions ?? []).flatMap(
      (group) => group.instructions,
    ),
  ];
  const records: KeyRecord[] = [];
  for (const instruction of instructions) {
    const target = instruction.accounts.at(0);
    if (
      keys.at(instruction.programIdIndex) !== USER_REGISTRY_PROGRAM_ID ||
      target === undefined ||
      keys.at(target) !== recordAddress
    ) {
      continue;
    }
    const record = setDataRecord(
      new Uint8Array(base58.encode(instruction.data)),
    );
    if (record !== undefined) records.push(record);
  }
  return records;
}

/** The key record in `set_data` instruction data, or undefined for any other. */
function setDataRecord(data: Uint8Array): KeyRecord | undefined {
  if (data.at(0) !== SET_DATA) return undefined;
  try {
    return decodeRecordData(data.subarray(1));
  } catch {
    return undefined;
  }
}

/**
 * Registry `register` of the vault with the record's public keys. The vault
 * owns and pays for the registry record. It fails on chain once the vault is
 * registered.
 */
export function registerVaultKeysInstruction(
  vault: Address,
  record: KeyRecord,
): Promise<Instruction> {
  return getRegisterInstructionAsync({
    owner: vault,
    payer: vault,
    nullifierPublicKey: record.nullifierPublicKey,
    viewingPublicKey: record.sharedViewingPublicKey,
  });
}

/**
 * Registry `update_keys`: replaces the viewing key of `owner`'s record and
 * fails on chain when `owner` has no record.
 */
export async function updateKeysInstruction(
  owner: Address,
  viewingPublicKey: Bytes33,
): Promise<Instruction> {
  return {
    programAddress: USER_REGISTRY_PROGRAM_ID,
    accounts: [
      {
        address: await getUserRecordAddress(owner),
        role: AccountRole.WRITABLE,
      },
      { address: owner, role: AccountRole.READONLY_SIGNER },
    ],
    data: new ByteWriter().u8(UPDATE_KEYS).bytes(viewingPublicKey).finish(),
  };
}

/**
 * Registry `set_data` with one key record, replacing `owner`'s record data;
 * `owner` also pays the rent the larger account is missing.
 */
export async function setKeyRecordInstruction(
  owner: Address,
  record: KeyRecord,
): Promise<Instruction> {
  const encoded = encodeKeyRecord(record);
  return {
    programAddress: USER_REGISTRY_PROGRAM_ID,
    accounts: [
      {
        address: await getUserRecordAddress(owner),
        role: AccountRole.WRITABLE,
      },
      { address: owner, role: AccountRole.READONLY_SIGNER },
      { address: owner, role: AccountRole.WRITABLE_SIGNER },
      { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
    ],
    data: new ByteWriter()
      .u8(SET_DATA)
      .u32(encoded.length)
      .bytes(encoded)
      .finish(),
  };
}

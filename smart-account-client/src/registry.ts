/**
 * Where the vault's keys live (design.md, "Key Record"). The user registry
 * record holds the registered shared viewing key; `register` creates it and
 * `update_keys` replaces it. The current key record lives in the vault's data
 * record of the data registry (program/), a stripped-down registry that keeps
 * only `set_data`. Both are written inside the same Squads transaction that
 * registers or rotates the keys. Writing needs the vault's signature, so only
 * a threshold of signers can set a record. `set_data` replaces the data, so
 * earlier records survive only in the transactions that set them.
 */
import {
  AccountRole,
  address,
  getAddressEncoder,
  getBase58Encoder,
  getProgramDerivedAddress,
  type Address,
  type Instruction,
  type Signature,
} from "@solana/kit";
import type { SprClient } from "@heliuslabs/spr/client";
import { getUserRecordAddress } from "@heliuslabs/spr/addresses";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { USER_REGISTRY_PROGRAM_ID } from "@heliuslabs/spr/interface";
import type { Bytes33 } from "@heliuslabs/spr/keypair";
import { fetchUserRecord } from "@heliuslabs/spr/wallet";

import { ByteReader, ByteWriter } from "./bytes.js";
import {
  decodeKeyRecord,
  encodeKeyRecord,
  isCurrentKeyRecord,
  type KeyRecord,
} from "./keyRecord.js";
import { SYSTEM_PROGRAM_ADDRESS } from "./squads.js";

/** The data registry program (program/), at the address the localnet loads it. */
export const DATA_REGISTRY_PROGRAM_ADDRESS = address(
  "Eoxe4CcAxAjM4FE4YcR889kDVDmZvr9txS74LQ8ybHdU",
);
const UPDATE_KEYS = 2;
const SET_DATA = 0;
const SIGNATURE_PAGE = 1000;
const DATA_SEED = new TextEncoder().encode("data");
const addressEncoder = getAddressEncoder();
const base58 = getBase58Encoder();

/** What the registries say about the vault's keys. */
export interface VaultRegistry {
  /** The registered shared viewing public key. */
  readonly viewingPublicKey: Bytes33;
  /** The key record in the vault's data record, if any. */
  readonly record: KeyRecord | undefined;
}

/** `owner`'s data record: the PDA `[b"data", owner]` of the data registry. */
async function dataRecordAddress(owner: Address): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: DATA_REGISTRY_PROGRAM_ADDRESS,
    seeds: [DATA_SEED, addressEncoder.encode(owner)],
  });
  return pda;
}

/** The vault's registered key and key record, or undefined before Register. */
export async function fetchVaultRegistry(
  client: SprClient,
  vault: Address,
): Promise<VaultRegistry | undefined> {
  const userRecord = await fetchUserRecord({ rpc: client, owner: vault });
  if (userRecord === undefined) return undefined;
  const { value } = await client.solanaRpc
    .getAccountInfo(await dataRecordAddress(vault), {
      encoding: "base64",
      commitment: "confirmed",
    })
    .send();
  const { viewingPublicKey } = userRecord;
  if (value === null) return { viewingPublicKey, record: undefined };
  if (value.owner !== DATA_REGISTRY_PROGRAM_ADDRESS) {
    throw new Error(`the data record of ${vault} has another owner program`);
  }
  return {
    viewingPublicKey,
    record: decodeRecordData(
      new Uint8Array(Buffer.from(value.data[0], "base64")),
    ),
  };
}

/**
 * The key record in a data record: a u32 length, then exactly one encoded
 * record. No data means no record.
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
 * Every key record a successful data registry `set_data` wrote to `vault`'s
 * data record, newest first, read from that account's transaction history.
 * It includes the current record. Only the vault can sign `set_data`, so each
 * record went through a threshold of signers. A `set_data` whose data does
 * not decode as a key record is skipped.
 */
export async function fetchKeyRecordHistory(
  client: SprClient,
  vault: Address,
): Promise<KeyRecord[]> {
  const recordAddress = await dataRecordAddress(vault);
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
      keys.at(instruction.programIdIndex) !== DATA_REGISTRY_PROGRAM_ADDRESS ||
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
 * Data registry `set_data` with one key record, replacing `owner`'s data
 * record and creating it on the first write; `owner` also pays the rent the
 * record is missing.
 */
export async function setKeyRecordInstruction(
  owner: Address,
  record: KeyRecord,
): Promise<Instruction> {
  const encoded = encodeKeyRecord(record);
  return {
    programAddress: DATA_REGISTRY_PROGRAM_ADDRESS,
    accounts: [
      { address: await dataRecordAddress(owner), role: AccountRole.WRITABLE },
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

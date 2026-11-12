/**
 * Key records uploaded in chunks and published by the vault.
 * Publication and user-registry changes run in the same Squads transaction.
 * Superseded records are closed; the latest record contains all old viewing keys.
 */
import {
  AccountRole,
  type Address,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import type { SprClient } from "@heliuslabs/spr/client";
import { getUserRecordAddress } from "@heliuslabs/spr/addresses";
import { getRegisterInstructionAsync } from "@heliuslabs/spr/instructions";
import { USER_REGISTRY_PROGRAM_ID } from "@heliuslabs/spr/interface";
import type { Bytes33 } from "@heliuslabs/spr/keypair";
import { fetchUserRecord } from "@heliuslabs/spr/wallet";
import { ByteWriter } from "./bytes.js";
import {
  decodeKeyRecord,
  encodeKeyRecord,
  isCurrentKeyRecord,
  type KeyRecord,
} from "./keyRecord.js";
import {
  applyRecordInstruction,
  fetchStorageHead,
  fetchStoredRecord,
  prepareRecordUpload,
  type RecordUpload,
} from "./storage.js";

const UPDATE_KEYS = 2;

/** What the registries say about the vault's keys. */
export interface VaultRegistry {
  /** The registered shared viewing public key. */
  readonly viewingPublicKey: Bytes33;
  /** The current record address used as the next update's predecessor. */
  readonly recordAddress: Address | undefined;
  /** Storage revision bound into the next signed apply instruction. */
  readonly recordRevision: bigint;
  /** The key record in the vault's data record, if any. */
  readonly record: KeyRecord | undefined;
}

/** The registered viewing key and current published record, if registered. */
export async function fetchVaultRegistry(
  client: SprClient,
  vault: Address,
): Promise<VaultRegistry | undefined> {
  const userRecord = await fetchUserRecord({ rpc: client, owner: vault });
  if (userRecord === undefined) return undefined;
  const current = await fetchStorageHead(client, vault);
  const record =
    current === undefined
      ? undefined
      : await publishedKeyRecord(client, vault, current.current);
  return {
    viewingPublicKey: userRecord.viewingPublicKey,
    recordAddress: current?.current,
    recordRevision: current?.revision ?? 0n,
    record,
  };
}

async function publishedKeyRecord(
  client: SprClient,
  owner: Address,
  address: Address,
): Promise<KeyRecord> {
  const stored = await fetchStoredRecord(client, address);
  if (!stored.sealed || stored.owner !== owner)
    throw new Error("record is unpublished or belongs to another vault");
  return decodeKeyRecord(stored.bytes);
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

/** Prepare upload instructions; send each before submitting register or rotate. */
export function prepareKeyRecordUpload(
  input: Readonly<{
    owner: Address;
    previous: Address;
    payer: TransactionSigner;
    record: KeyRecord;
  }>,
): Promise<RecordUpload> {
  return prepareRecordUpload({
    ...input,
    bytes: encodeKeyRecord(input.record),
  });
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

/** The vault publishes the uploaded record after checking its exact bytes. */
export function applyKeyRecordInstruction(
  owner: Address,
  buffer: Address,
  previous: Address,
  previousRevision: bigint,
  record: KeyRecord,
): Promise<Instruction> {
  return applyRecordInstruction(
    owner,
    buffer,
    previous,
    previousRevision,
    encodeKeyRecord(record),
  );
}

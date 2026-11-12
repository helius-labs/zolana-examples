/** Chunked uploads and atomic replacement through a small head PDA. */
import { createHash } from "node:crypto";
import {
  AccountRole,
  address,
  generateKeyPairSigner,
  getAddressDecoder,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type AccountSignerMeta,
  type Instruction,
  type TransactionSigner,
} from "@solana/kit";
import type { SprClient } from "@heliuslabs/spr/client";
import { ByteReader, ByteWriter, bytesEqual } from "./bytes.js";
import { SYSTEM_PROGRAM_ADDRESS } from "./squads.js";

export const STORAGE_PROGRAM_ADDRESS = address(
  "Stor9L8wQBAGcAcoUcADt5sWYYaEA65Ws2Cgt5UUqXj",
);
export const STORAGE_CHUNK_SIZE = 1024;
export const BUFFER_HEADER_SIZE = 109;
export const MAX_RECORD_SIZE = 10 * 1024 * 1024 - BUFFER_HEADER_SIZE;
export const EMPTY_RECORD_ADDRESS = SYSTEM_PROGRAM_ADDRESS;
const addressEncoder = getAddressEncoder();
const addressDecoder = getAddressDecoder();
const utf8 = new TextEncoder();
const BUFFER_DISCRIMINATOR = utf8.encode("STORBUF1");
const HEAD_DISCRIMINATOR = utf8.encode("STORHEAD");

export interface StoredRecord {
  readonly sealed: boolean;
  readonly owner: Address;
  /** The apply precondition; this predecessor is closed after replacement. */
  readonly previous: Address;
  readonly totalLength: number;
  readonly dataHash: Uint8Array;
  readonly bytes: Uint8Array;
}

export interface RecordUpload {
  readonly address: Address;
  readonly previous: Address;
  readonly instructions: readonly Instruction[];
  readonly closeInstruction: Instruction;
}

export interface StorageHead {
  readonly current: Address;
  readonly revision: bigint;
}

export async function storageHeadAddress(owner: Address): Promise<Address> {
  const [head] = await getProgramDerivedAddress({
    programAddress: STORAGE_PROGRAM_ADDRESS,
    seeds: [utf8.encode("head"), addressEncoder.encode(owner)],
  });
  return head;
}

export function decodeStorageHead(bytes: Uint8Array): StorageHead {
  const reader = new ByteReader(bytes, "storage head");
  if (!bytesEqual(reader.bytes(8, "discriminator"), HEAD_DISCRIMINATOR)) {
    throw new Error("not a storage head");
  }
  const current = addressDecoder.decode(reader.bytes(32, "current record"));
  const revision = reader.u64("revision");
  reader.end();
  if (current === EMPTY_RECORD_ADDRESS || revision === 0n)
    throw new Error("empty storage head");
  return { current, revision };
}

export function decodeStoredRecord(bytes: Uint8Array): StoredRecord {
  const reader = new ByteReader(bytes, "stored record");
  if (!bytesEqual(reader.bytes(8, "discriminator"), BUFFER_DISCRIMINATOR)) {
    throw new Error("not a stored record");
  }
  const state = reader.u8("sealed");
  if (state !== 0 && state !== 1)
    throw new Error("invalid stored record state");
  const owner = addressDecoder.decode(reader.bytes(32, "owner"));
  const previous = addressDecoder.decode(reader.bytes(32, "previous"));
  const totalLength = reader.u32("total length");
  const dataHash = reader.bytes(32, "data hash");
  const uploaded = reader.bytes(bytes.length - BUFFER_HEADER_SIZE, "bytes");
  reader.end();
  if (
    totalLength === 0 ||
    totalLength > MAX_RECORD_SIZE ||
    uploaded.length > totalLength ||
    (state === 1 && uploaded.length !== totalLength)
  ) {
    throw new Error("invalid stored record length");
  }
  return {
    sealed: state === 1,
    owner,
    previous,
    totalLength,
    dataHash,
    bytes: uploaded,
  };
}

export async function fetchStorageHead(
  client: SprClient,
  owner: Address,
): Promise<StorageHead | undefined> {
  const account = await client.getAccount(await storageHeadAddress(owner));
  if (account === undefined) return undefined;
  if (account.owner !== STORAGE_PROGRAM_ADDRESS)
    throw new Error("wrong storage head owner");
  return decodeStorageHead(account.data);
}

export async function fetchStoredRecord(
  client: SprClient,
  record: Address,
): Promise<StoredRecord> {
  const account = await client.getAccount(record);
  if (account === undefined || account.owner !== STORAGE_PROGRAM_ADDRESS) {
    throw new Error("missing record or wrong storage program owner");
  }
  return decodeStoredRecord(account.data);
}

/** The hash approved by apply, using the same chunk boundaries as the upload. */
export function storageDataHash(bytes: Uint8Array): Uint8Array {
  let dataHash = new Uint8Array(32);
  for (let offset = 0; offset < bytes.length; offset += STORAGE_CHUNK_SIZE) {
    dataHash = Uint8Array.from(
      createHash("sha256")
        .update(dataHash)
        .update(bytes.subarray(offset, offset + STORAGE_CHUNK_SIZE))
        .digest(),
    );
  }
  return dataHash;
}

/** Upload separately; publication requires the owner's signature in apply. */
export async function prepareRecordUpload(
  input: Readonly<{
    owner: Address;
    previous: Address;
    payer: TransactionSigner;
    bytes: Uint8Array;
  }>,
): Promise<RecordUpload> {
  if (input.bytes.length === 0 || input.bytes.length > MAX_RECORD_SIZE) {
    throw new Error(
      "record length must be between 1 byte and the account limit",
    );
  }
  const buffer = await generateKeyPairSigner();
  const bufferAccount = {
    address: buffer.address,
    role: AccountRole.WRITABLE_SIGNER,
    signer: buffer,
  } satisfies AccountSignerMeta;
  const payerAccount = {
    address: input.payer.address,
    role: AccountRole.WRITABLE_SIGNER,
    signer: input.payer,
  } satisfies AccountSignerMeta;
  const instructions: Instruction[] = [];
  for (
    let offset = 0;
    offset < input.bytes.length;
    offset += STORAGE_CHUNK_SIZE
  ) {
    instructions.push({
      programAddress: STORAGE_PROGRAM_ADDRESS,
      accounts: [
        bufferAccount,
        payerAccount,
        { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      ],
      data: new ByteWriter()
        .u8(1)
        .bytes(addressEncoder.encode(input.owner))
        .bytes(addressEncoder.encode(input.previous))
        .u32(input.bytes.length)
        .u32(offset)
        .bytes(input.bytes.subarray(offset, offset + STORAGE_CHUNK_SIZE))
        .finish(),
    });
  }
  return {
    address: buffer.address,
    previous: input.previous,
    instructions,
    closeInstruction: {
      programAddress: STORAGE_PROGRAM_ADDRESS,
      accounts: [
        bufferAccount,
        { address: input.payer.address, role: AccountRole.WRITABLE },
      ],
      data: Uint8Array.of(3),
    },
  };
}

/** Publishes exactly the bytes the owner approved, with stale-head protection. */
export async function applyRecordInstruction(
  owner: Address,
  buffer: Address,
  previous: Address,
  previousRevision: bigint,
  bytes: Uint8Array,
): Promise<Instruction> {
  if (bytes.length === 0 || bytes.length > MAX_RECORD_SIZE)
    throw new Error("invalid record length");
  return {
    programAddress: STORAGE_PROGRAM_ADDRESS,
    accounts: [
      { address: await storageHeadAddress(owner), role: AccountRole.WRITABLE },
      { address: buffer, role: AccountRole.WRITABLE },
      { address: owner, role: AccountRole.READONLY_SIGNER },
      { address: owner, role: AccountRole.WRITABLE_SIGNER },
      { address: SYSTEM_PROGRAM_ADDRESS, role: AccountRole.READONLY },
      {
        address: previous,
        role:
          previous === EMPTY_RECORD_ADDRESS
            ? AccountRole.READONLY
            : AccountRole.WRITABLE,
      },
    ],
    data: new ByteWriter()
      .u8(2)
      .bytes(addressEncoder.encode(previous))
      .u64(previousRevision)
      .bytes(storageDataHash(bytes))
      .finish(),
  };
}

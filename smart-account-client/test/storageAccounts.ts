/** Fake storage-program accounts for unit tests that read through a client. */
import { getAddressEncoder, type Address } from "@solana/kit";
import type { SprClient } from "@heliuslabs/spr/client";
import { ByteWriter } from "../src/bytes.js";
import {
  STORAGE_PROGRAM_ADDRESS,
  storageDataHash,
  storageHeadAddress,
} from "../src/storage.js";

export type FakeAccounts = Map<Address, { owner: Address; data: Uint8Array }>;

const addressEncoder = getAddressEncoder();
const utf8 = new TextEncoder();

/** A storage buffer account: header, then the uploaded bytes. */
export function storedBytes(
  input: Readonly<{
    owner: Address;
    payer: Address;
    /** The payer unless given. */
    refund?: Address;
    previous: Address;
    bytes: Uint8Array;
    sealed?: number;
    totalLength?: number;
  }>,
): Uint8Array {
  return new ByteWriter()
    .bytes(utf8.encode("STORBUF1"))
    .u8(input.sealed ?? 1)
    .bytes(addressEncoder.encode(input.owner))
    .bytes(addressEncoder.encode(input.payer))
    .bytes(addressEncoder.encode(input.refund ?? input.payer))
    .bytes(addressEncoder.encode(input.previous))
    .u32(input.totalLength ?? input.bytes.length)
    .bytes(storageDataHash(input.bytes))
    .bytes(input.bytes)
    .finish();
}

export function headBytes(current: Address, revision = 1n): Uint8Array {
  return new ByteWriter()
    .bytes(utf8.encode("STORHEAD"))
    .bytes(addressEncoder.encode(current))
    .u64(revision)
    .finish();
}

/** A client whose `getAccount` reads from `accounts`. */
export function accountClient(accounts: FakeAccounts): SprClient {
  return {
    getAccount: async (address: Address) => accounts.get(address),
  } as unknown as SprClient;
}

/** Adds `owner`'s head and the sealed record it points to. */
export async function publish(
  accounts: FakeAccounts,
  input: Readonly<{
    owner: Address;
    record: Address;
    payer: Address;
    refund?: Address;
    previous: Address;
    bytes: Uint8Array;
    revision?: bigint;
  }>,
): Promise<void> {
  accounts.set(await storageHeadAddress(input.owner), {
    owner: STORAGE_PROGRAM_ADDRESS,
    data: headBytes(input.record, input.revision),
  });
  accounts.set(input.record, {
    owner: STORAGE_PROGRAM_ADDRESS,
    data: storedBytes(input),
  });
}

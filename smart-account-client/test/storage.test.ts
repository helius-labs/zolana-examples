import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { describe, it } from "node:test";
import {
  AccountRole,
  generateKeyPairSigner,
  getAddressEncoder,
  getAddressDecoder,
  type Address,
} from "@solana/kit";
import type { SprClient } from "@heliuslabs/spr/client";
import { ByteReader, ByteWriter } from "../src/bytes.js";
import {
  STORAGE_PROGRAM_ADDRESS,
  STORAGE_CHUNK_SIZE,
  BUFFER_HEADER_SIZE,
  EMPTY_RECORD_ADDRESS,
  applyRecordInstruction,
  decodeStorageHead,
  decodeStoredRecord,
  fetchStorageHead,
  fetchStoredRecord,
  prepareRecordUpload,
  storageDataHash,
  storageHeadAddress,
} from "../src/storage.js";

const addressEncoder = getAddressEncoder();
const addressDecoder = getAddressDecoder();
const utf8 = new TextEncoder();
const RECORD_MAGIC = utf8.encode("STORBUF1");

function storedBytes(
  owner: Address,
  previous: Address,
  bytes: Uint8Array,
  sealed = 1,
  totalLength = bytes.length,
): Uint8Array {
  return new ByteWriter()
    .bytes(RECORD_MAGIC)
    .u8(sealed)
    .bytes(addressEncoder.encode(owner))
    .bytes(addressEncoder.encode(previous))
    .u32(totalLength)
    .bytes(storageDataHash(bytes))
    .bytes(bytes)
    .finish();
}

function headBytes(current: Address, revision = 1n): Uint8Array {
  return new ByteWriter()
    .bytes(utf8.encode("STORHEAD"))
    .bytes(addressEncoder.encode(current))
    .u64(revision)
    .finish();
}

function accountClient(
  accounts: Map<Address, { owner: Address; data: Uint8Array }>,
): SprClient {
  return {
    getAccount: async (address: Address) => accounts.get(address),
  } as unknown as SprClient;
}

describe("versioned storage", () => {
  it("uploads bounded, contiguous chunks and approves exactly their hash", async () => {
    const payer = await generateKeyPairSigner();
    const owner = (await generateKeyPairSigner()).address;
    const bytes = Uint8Array.from({ length: 2800 }, (_, index) => index % 251);
    const upload = await prepareRecordUpload({
      owner,
      previous: EMPTY_RECORD_ADDRESS,
      payer,
      bytes,
    });
    const chunks: Uint8Array[] = [];
    let written = 0;
    let dataHash: Uint8Array = new Uint8Array(32);
    for (const instruction of upload.instructions) {
      const reader = new ByteReader(
        Uint8Array.from(instruction.data!),
        "store",
      );
      assert.equal(reader.u8("tag"), 1);
      assert.equal(addressDecoder.decode(reader.bytes(32, "owner")), owner);
      assert.equal(
        addressDecoder.decode(reader.bytes(32, "predecessor")),
        EMPTY_RECORD_ADDRESS,
      );
      assert.equal(reader.u32("length"), bytes.length);
      assert.equal(reader.u32("offset"), written);
      const chunk = reader.bytes(instruction.data!.length - 73, "chunk");
      reader.end();
      assert(chunk.length > 0 && chunk.length <= STORAGE_CHUNK_SIZE);
      chunks.push(chunk);
      written += chunk.length;
      dataHash = Uint8Array.from(
        createHash("sha256").update(dataHash).update(chunk).digest(),
      );
      assert.deepEqual(
        instruction.accounts?.map(({ address }) => address),
        [upload.address, payer.address, EMPTY_RECORD_ADDRESS],
      );
    }
    assert.equal(upload.instructions.length, 3);
    assert.deepEqual(Uint8Array.from(Buffer.concat(chunks)), bytes);
    const apply = await applyRecordInstruction(
      owner,
      upload.address,
      EMPTY_RECORD_ADDRESS,
      0n,
      bytes,
    );
    assert.deepEqual(
      Uint8Array.from(apply.data!),
      new ByteWriter()
        .u8(2)
        .bytes(addressEncoder.encode(EMPTY_RECORD_ADDRESS))
        .u64(0n)
        .bytes(dataHash)
        .finish(),
    );
    assert.equal(
      apply.accounts?.at(0)?.address,
      await storageHeadAddress(owner),
    );
    assert.equal(apply.accounts?.at(1)?.address, upload.address);
    assert.deepEqual(
      Uint8Array.from(upload.closeInstruction.data!),
      Uint8Array.of(3),
    );
  });

  it("matches the independent SHA-256 vector used by the SBF tests", () => {
    assert.equal(
      Buffer.from(storageDataHash(new Uint8Array(25_000).fill(37))).toString(
        "hex",
      ),
      "369777a38d9a2dc6382c3da6dadf3361f1fbe4b8bbcf7d0d49fc7e15b7e78353",
    );
  });

  it("distinguishes partial uploads from complete sealed records", async () => {
    const owner = (await generateKeyPairSigner()).address;
    const bytes = Uint8Array.of(1, 2, 3);
    assert.deepEqual(
      decodeStoredRecord(storedBytes(owner, EMPTY_RECORD_ADDRESS, bytes)),
      {
        sealed: true,
        owner,
        previous: EMPTY_RECORD_ADDRESS,
        totalLength: 3,
        dataHash: storageDataHash(bytes),
        bytes,
      },
    );
    assert.equal(
      decodeStoredRecord(storedBytes(owner, EMPTY_RECORD_ADDRESS, bytes, 0, 5))
        .sealed,
      false,
    );
    assert.throws(
      () =>
        decodeStoredRecord(
          storedBytes(owner, EMPTY_RECORD_ADDRESS, bytes, 1, 5),
        ),
      /length/,
    );
    assert.throws(
      () =>
        decodeStoredRecord(storedBytes(owner, EMPTY_RECORD_ADDRESS, bytes, 2)),
      /state/,
    );
    assert.throws(
      () =>
        decodeStoredRecord(
          storedBytes(owner, EMPTY_RECORD_ADDRESS, bytes, 0, 2),
        ),
      /length/,
    );
    const encoded = storedBytes(owner, EMPTY_RECORD_ADDRESS, bytes);
    for (let length = 0; length < BUFFER_HEADER_SIZE; length += 1) {
      assert.throws(
        () => decodeStoredRecord(encoded.slice(0, length)),
        /truncated/,
      );
    }
    assert.throws(
      () => decodeStorageHead(new Uint8Array(48)),
      /not a storage head/,
    );
    assert.throws(
      () => decodeStorageHead(headBytes(EMPTY_RECORD_ADDRESS)),
      /empty storage head/,
    );
  });

  it("reads the current record without needing a closed predecessor", async () => {
    const owner = (await generateKeyPairSigner()).address;
    const previous = (await generateKeyPairSigner()).address;
    const current = (await generateKeyPairSigner()).address;
    const head = await storageHeadAddress(owner);
    const bytes = Uint8Array.of(4, 5, 6);
    const accounts = new Map<Address, { owner: Address; data: Uint8Array }>([
      [head, { owner: STORAGE_PROGRAM_ADDRESS, data: headBytes(current, 7n) }],
      [
        current,
        {
          owner: STORAGE_PROGRAM_ADDRESS,
          data: storedBytes(owner, previous, bytes),
        },
      ],
    ]);
    const client = accountClient(accounts);
    assert.deepEqual(await fetchStorageHead(client, owner), {
      current,
      revision: 7n,
    });
    assert.deepEqual((await fetchStoredRecord(client, current)).bytes, bytes);
    await assert.rejects(fetchStoredRecord(client, previous), /missing record/);
    accounts.delete(head);
    assert.equal(await fetchStorageHead(client, owner), undefined);
  });

  it("binds the previous account and revision into the signed apply", async () => {
    const owner = (await generateKeyPairSigner()).address;
    const previous = (await generateKeyPairSigner()).address;
    const current = (await generateKeyPairSigner()).address;
    const bytes = Uint8Array.of(7);
    const instruction = await applyRecordInstruction(
      owner,
      current,
      previous,
      7n,
      bytes,
    );
    const reader = new ByteReader(Uint8Array.from(instruction.data!), "apply");
    assert.equal(reader.u8("tag"), 2);
    assert.equal(addressDecoder.decode(reader.bytes(32, "previous")), previous);
    assert.equal(reader.u64("revision"), 7n);
    assert.deepEqual(reader.bytes(32, "hash"), storageDataHash(bytes));
    reader.end();
    assert.deepEqual(instruction.accounts?.at(5), {
      address: previous,
      role: AccountRole.WRITABLE,
    });
  });

  it("rejects foreign owners and invalid head revisions", async () => {
    const owner = (await generateKeyPairSigner()).address;
    const record = (await generateKeyPairSigner()).address;
    const head = await storageHeadAddress(owner);
    const accounts = new Map<Address, { owner: Address; data: Uint8Array }>([
      [head, { owner: EMPTY_RECORD_ADDRESS, data: headBytes(record) }],
      [
        record,
        {
          owner: EMPTY_RECORD_ADDRESS,
          data: storedBytes(owner, EMPTY_RECORD_ADDRESS, Uint8Array.of(1)),
        },
      ],
    ]);
    const client = accountClient(accounts);
    await assert.rejects(
      fetchStorageHead(client, owner),
      /wrong storage head owner/,
    );
    await assert.rejects(
      fetchStoredRecord(client, record),
      /wrong storage program owner/,
    );
    assert.throws(
      () => decodeStorageHead(headBytes(record, 0n)),
      /empty storage head/,
    );
  });

  it("refuses an empty upload before preparing instructions", async () => {
    const payer = await generateKeyPairSigner();
    await assert.rejects(
      prepareRecordUpload({
        owner: payer.address,
        previous: EMPTY_RECORD_ADDRESS,
        payer,
        bytes: new Uint8Array(),
      }),
      /record length/,
    );
  });
});

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
import { ByteReader, ByteWriter } from "../src/bytes.js";
import {
  STORAGE_PROGRAM_ADDRESS,
  STORAGE_CHUNK_SIZE,
  BUFFER_HEADER_SIZE,
  EMPTY_RECORD_ADDRESS,
  applyRecordInstruction,
  decodeStorageHead,
  decodeStoredRecord,
  fetchPublishedRecord,
  fetchStorageHead,
  fetchStoredRecord,
  prepareRecordUpload,
  storageDataHash,
  storageHeadAddress,
} from "../src/storage.js";
import {
  accountClient,
  headBytes,
  publish,
  storedBytes,
  type FakeAccounts,
} from "./storageAccounts.js";

const addressEncoder = getAddressEncoder();
const addressDecoder = getAddressDecoder();
/** Tag, owner, predecessor, total length and offset before each chunk. */
const STORE_PREFIX = 1 + 32 + 32 + 4 + 4;

async function newAddress(): Promise<Address> {
  return (await generateKeyPairSigner()).address;
}

describe("versioned storage", () => {
  it("uploads canonical chunks and approves exactly their hash", async () => {
    const payer = await generateKeyPairSigner();
    const owner = await newAddress();
    const refund = await newAddress();
    const bytes = Uint8Array.from({ length: 2800 }, (_, index) => index % 251);
    const upload = await prepareRecordUpload({
      owner,
      previous: EMPTY_RECORD_ADDRESS,
      payer,
      refund,
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
      const chunk = reader.bytes(
        instruction.data!.length - STORE_PREFIX,
        "chunk",
      );
      reader.end();
      chunks.push(chunk);
      written += chunk.length;
      dataHash = Uint8Array.from(
        createHash("sha256").update(dataHash).update(chunk).digest(),
      );
      assert.deepEqual(
        instruction.accounts?.map(({ address }) => address),
        [upload.address, payer.address, EMPTY_RECORD_ADDRESS, refund],
      );
    }
    // The program accepts a short chunk only at the end.
    assert.deepEqual(
      chunks.map((chunk) => chunk.length),
      [STORAGE_CHUNK_SIZE, STORAGE_CHUNK_SIZE, STORAGE_CHUNK_SIZE, 496],
    );
    assert.deepEqual(Uint8Array.from(Buffer.concat(chunks)), bytes);
    const apply = await applyRecordInstruction(
      owner,
      upload.address,
      undefined,
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
    assert.deepEqual(
      apply.accounts?.map(({ address, role }) => ({ address, role })),
      [
        {
          address: await storageHeadAddress(owner),
          role: AccountRole.WRITABLE,
        },
        { address: upload.address, role: AccountRole.WRITABLE },
        { address: owner, role: AccountRole.READONLY_SIGNER },
        { address: owner, role: AccountRole.WRITABLE_SIGNER },
        { address: EMPTY_RECORD_ADDRESS, role: AccountRole.READONLY },
        { address: EMPTY_RECORD_ADDRESS, role: AccountRole.READONLY },
        { address: EMPTY_RECORD_ADDRESS, role: AccountRole.READONLY },
      ],
      "the first apply closes nothing and refunds no one",
    );
    assert.deepEqual(
      {
        accounts: upload.closeInstruction.accounts?.map(
          ({ address, role }) => ({ address, role }),
        ),
        data: Uint8Array.from(upload.closeInstruction.data!),
      },
      {
        accounts: [
          { address: upload.address, role: AccountRole.WRITABLE },
          { address: payer.address, role: AccountRole.READONLY_SIGNER },
          { address: refund, role: AccountRole.WRITABLE },
        ],
        data: Uint8Array.of(3),
      },
      "the rent payer cancels the upload and its destination is refunded",
    );
  });

  it("matches the independent SHA-256 vector used by the SBF tests", () => {
    assert.equal(
      Buffer.from(storageDataHash(new Uint8Array(25_000).fill(37))).toString(
        "hex",
      ),
      "3c3fe9a628e835a2691694e2a3ccf8f7e42c104fe99a90d64c1de10b4ff8e03e",
    );
  });

  it("distinguishes partial uploads from complete sealed records", async () => {
    const owner = await newAddress();
    const payer = await newAddress();
    const refund = await newAddress();
    const bytes = Uint8Array.of(1, 2, 3);
    const stored = (sealed: number, totalLength?: number) =>
      storedBytes({
        owner,
        payer,
        refund,
        previous: EMPTY_RECORD_ADDRESS,
        bytes,
        sealed,
        totalLength,
      });
    assert.deepEqual(decodeStoredRecord(stored(1)), {
      sealed: true,
      owner,
      payer,
      refund,
      previous: EMPTY_RECORD_ADDRESS,
      totalLength: 3,
      dataHash: storageDataHash(bytes),
      bytes,
    });
    assert.equal(decodeStoredRecord(stored(0, 5)).sealed, false);
    assert.throws(() => decodeStoredRecord(stored(1, 5)), /length/);
    assert.throws(() => decodeStoredRecord(stored(2)), /state/);
    assert.throws(() => decodeStoredRecord(stored(0, 2)), /length/);
    const encoded = stored(1);
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

  it("reads the published record without needing a closed predecessor", async () => {
    const owner = await newAddress();
    const payer = await newAddress();
    const refund = await newAddress();
    const previous = await newAddress();
    const current = await newAddress();
    const bytes = Uint8Array.of(4, 5, 6);
    const accounts: FakeAccounts = new Map();
    await publish(accounts, {
      owner,
      record: current,
      payer,
      refund,
      previous,
      bytes,
      revision: 7n,
    });
    const client = accountClient(accounts);
    assert.deepEqual(await fetchPublishedRecord(client, owner), {
      address: current,
      revision: 7n,
      refund,
      bytes,
    });
    await assert.rejects(fetchStoredRecord(client, previous), /missing record/);
    accounts.delete(await storageHeadAddress(owner));
    assert.equal(await fetchStorageHead(client, owner), undefined);
    assert.equal(await fetchPublishedRecord(client, owner), undefined);
  });

  it("rejects a head that points to an unpublished or foreign record", async () => {
    const owner = await newAddress();
    const payer = await newAddress();
    const record = await newAddress();
    const accounts: FakeAccounts = new Map();
    await publish(accounts, {
      owner,
      record,
      payer,
      previous: EMPTY_RECORD_ADDRESS,
      bytes: Uint8Array.of(1),
    });
    const client = accountClient(accounts);
    for (const data of [
      storedBytes({
        owner,
        payer,
        previous: EMPTY_RECORD_ADDRESS,
        bytes: Uint8Array.of(1),
        sealed: 0,
      }),
      storedBytes({
        owner: payer,
        payer,
        previous: EMPTY_RECORD_ADDRESS,
        bytes: Uint8Array.of(1),
      }),
    ]) {
      accounts.set(record, { owner: STORAGE_PROGRAM_ADDRESS, data });
      await assert.rejects(
        fetchPublishedRecord(client, owner),
        /unpublished or foreign record/,
      );
    }
  });

  it("binds the predecessor and revision and refunds its destination", async () => {
    const owner = await newAddress();
    const previous = await newAddress();
    const refund = await newAddress();
    const current = await newAddress();
    const bytes = Uint8Array.of(7);
    const instruction = await applyRecordInstruction(
      owner,
      current,
      { address: previous, revision: 7n, refund },
      bytes,
    );
    const reader = new ByteReader(Uint8Array.from(instruction.data!), "apply");
    assert.equal(reader.u8("tag"), 2);
    assert.equal(addressDecoder.decode(reader.bytes(32, "previous")), previous);
    assert.equal(reader.u64("revision"), 7n);
    assert.deepEqual(reader.bytes(32, "hash"), storageDataHash(bytes));
    reader.end();
    assert.deepEqual(instruction.accounts?.slice(5), [
      { address: previous, role: AccountRole.WRITABLE },
      { address: refund, role: AccountRole.WRITABLE },
    ]);
  });

  it("rejects foreign owners and invalid head revisions", async () => {
    const owner = await newAddress();
    const record = await newAddress();
    const head = await storageHeadAddress(owner);
    const accounts: FakeAccounts = new Map([
      [head, { owner: EMPTY_RECORD_ADDRESS, data: headBytes(record) }],
      [
        record,
        {
          owner: EMPTY_RECORD_ADDRESS,
          data: storedBytes({
            owner,
            payer: owner,
            previous: EMPTY_RECORD_ADDRESS,
            bytes: Uint8Array.of(1),
          }),
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

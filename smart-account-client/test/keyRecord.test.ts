import assert from "node:assert/strict";
import { before, describe, it } from "node:test";

import { generateKeyPairSigner, type Address } from "@solana/kit";
import {
  ShieldedKeypair,
  ViewingKey,
  initializePoseidon,
} from "@heliuslabs/spr/keypair";

import {
  buildKeyRecord,
  compareAddresses,
  decodeKeyRecord,
  encodeKeyRecord,
  generateNullifierKey,
  recoverSharedSecrets,
  type KeyRecord,
} from "../src/keyRecord.js";
import { ByteWriter } from "../src/bytes.js";
import { currentKeyRecord, decodeRecordData } from "../src/registry.js";

interface Member {
  readonly address: Address;
  readonly keypair: ShieldedKeypair;
}

function member(): Member {
  const keypair = ShieldedKeypair.generate();
  return { address: keypair.shieldedAddress().solanaAddress(), keypair };
}

async function settingsAddress(): Promise<Address> {
  return (await generateKeyPairSigner()).address;
}

function recordFor(settings: Address, members: readonly Member[], epoch = 0n) {
  const viewingKey = ViewingKey.generate();
  const nullifierKey = generateNullifierKey();
  const record = buildKeyRecord({
    settings,
    epoch,
    secrets: { viewingKey, nullifierKey },
    recipients: members.map(({ address, keypair }) => ({
      key: address,
      viewingPublicKey: keypair.viewingPublicKey(),
    })),
  });
  return { record, viewingKey, nullifierKey };
}

describe("key record", () => {
  before(async () => {
    await initializePoseidon();
  });

  it("every signer recovers both shared secrets", async () => {
    const members = [member(), member(), member()];
    const { record, viewingKey, nullifierKey } = recordFor(
      await settingsAddress(),
      members,
    );
    for (const { address, keypair } of members) {
      const secrets = recoverSharedSecrets(record, address, keypair);
      assert.deepEqual(
        secrets.viewingKey.secretBytes(),
        viewingKey.secretBytes(),
      );
      assert.deepEqual(
        secrets.nullifierKey.secretBytes(),
        nullifierKey.secretBytes(),
      );
    }
  });

  it("round-trips through the byte layout at 173 + 97 * N bytes", async () => {
    const members = [member(), member()];
    const { record } = recordFor(await settingsAddress(), members, 7n);
    const encoded = encodeKeyRecord(record);
    assert.equal(encoded.length, 173 + 97 * 2);
    assert.deepEqual(decodeKeyRecord(encoded), record);
  });

  it("sorts entries by signer", async () => {
    const members = [member(), member(), member(), member()];
    const { record } = recordFor(await settingsAddress(), members);
    const signers = record.entries.map((entry) => entry.signer);
    assert.deepEqual(signers, [...signers].sort(compareAddresses));
  });

  it("a signer detects a garbage ciphertext in its entry", async () => {
    const members = [member(), member()];
    const { record } = recordFor(await settingsAddress(), members);
    const target = members[0]!;
    const tampered: KeyRecord = {
      ...record,
      entries: record.entries.map((entry) =>
        entry.signer === target.address
          ? { ...entry, ciphertext: entry.ciphertext.map((byte) => byte ^ 1) }
          : entry,
      ),
    } as KeyRecord;
    assert.throws(
      () => recoverSharedSecrets(tampered, target.address, target.keypair),
      /shared viewing secret does not match/,
    );
    const other = members[1]!;
    assert.doesNotThrow(() =>
      recoverSharedSecrets(tampered, other.address, other.keypair),
    );
  });

  it("every signer detects a garbage nullifier ciphertext", async () => {
    const members = [member(), member()];
    const { record } = recordFor(await settingsAddress(), members);
    const tampered = {
      ...record,
      nullifierCiphertext: record.nullifierCiphertext.map((byte) => byte ^ 1),
    } as KeyRecord;
    for (const { address, keypair } of members) {
      assert.throws(
        () => recoverSharedSecrets(tampered, address, keypair),
        /nullifier secret does not match/,
      );
    }
  });

  it("a non-signer has no entry", async () => {
    const { record } = recordFor(await settingsAddress(), [member()]);
    const outsider = member();
    assert.throws(
      () => recoverSharedSecrets(record, outsider.address, outsider.keypair),
      /has no entry/,
    );
  });

  it("decodes the one key record in registry data", async () => {
    const { record } = recordFor(await settingsAddress(), [member()]);
    const encoded = [...encodeKeyRecord(record)];
    const data = (bytes: number[], length = bytes.length) =>
      new ByteWriter().u32(length).bytes(bytes).finish();

    assert.equal(decodeRecordData(new Uint8Array()), undefined);
    assert.deepEqual(decodeRecordData(data(encoded)), record);
    assert.throws(
      () => decodeRecordData(data(encoded.slice(0, -1))),
      /truncated/,
    );
    assert.throws(
      () => decodeRecordData(data([...encoded, 0], encoded.length)),
      /trailing bytes/,
    );
    assert.throws(() => decodeRecordData(data([...encoded, 0])), /trailing/);
  });

  it("the current record must match the signers and the registered key", async () => {
    const settings = await settingsAddress();
    const [a, b, c] = [member(), member(), member()];
    const first = recordFor(settings, [a, b], 0n).record;
    const second = recordFor(settings, [a, c], 1n).record;
    const signers = [a, c].map(({ address }) => address);
    const registry = {
      viewingPublicKey: second.sharedViewingPublicKey,
      record: second,
    };
    assert.deepEqual(currentKeyRecord(registry, settings, signers), second);

    const mismatches: [string, () => KeyRecord, RegExp][] = [
      [
        "a signer set the record does not name",
        () => currentKeyRecord(registry, settings, [a.address, b.address]),
        /does not match/,
      ],
      [
        "a registered key other than the record's",
        () =>
          currentKeyRecord(
            { ...registry, viewingPublicKey: first.sharedViewingPublicKey },
            settings,
            signers,
          ),
        /does not match/,
      ],
      [
        "another Settings account",
        () => currentKeyRecord(registry, a.address, signers),
        /does not match/,
      ],
      [
        "no record",
        () =>
          currentKeyRecord(
            { ...registry, record: undefined },
            settings,
            signers,
          ),
        /no key record/,
      ],
    ];
    for (const [name, lookup, error] of mismatches) {
      assert.throws(lookup, error, name);
    }
  });
});

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
  keyRecordSize,
  recoverSharedSecrets,
  type KeyRecord,
} from "../src/keyRecord.js";
import { InMemoryKeyRecordStore, currentKeyRecord } from "../src/store.js";

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
    assert.equal(encoded.length, keyRecordSize(2));
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
          ? {
              ...entry,
              ciphertext: entry.ciphertext.map(
                (byte) => byte ^ 1,
              ) as typeof entry.ciphertext,
            }
          : entry,
      ),
    };
    assert.throws(
      () => recoverSharedSecrets(tampered, target.address, target.keypair),
      /does not match the key record/,
    );
    const other = members[1]!;
    assert.doesNotThrow(() =>
      recoverSharedSecrets(tampered, other.address, other.keypair),
    );
  });

  it("a non-signer has no entry", async () => {
    const { record } = recordFor(await settingsAddress(), [member()]);
    const outsider = member();
    assert.throws(
      () => recoverSharedSecrets(record, outsider.address, outsider.keypair),
      /has no entry/,
    );
  });

  it("after registration a copy under a higher epoch makes the lookup fail", async () => {
    const settings = await settingsAddress();
    const [a, b] = [member(), member()];
    const attacker = member();
    const store = new InMemoryKeyRecordStore();
    const { record } = recordFor(settings, [a, b], 0n);
    await store.put(record);
    // Same public fields, a higher epoch, and b's entry pointing at the attacker.
    await store.put({
      ...record,
      epoch: 1n,
      entries: record.entries.map((entry) =>
        entry.signer === b.address
          ? {
              ...entry,
              viewingPublicKey: attacker.keypair.viewingPublicKey().toBytes(),
            }
          : entry,
      ),
    });
    await store.put(record); // an identical re-upload is not a conflict
    await assert.rejects(
      currentKeyRecord(store, {
        settings,
        signers: [a.address, b.address],
        registeredViewingPublicKey: record.sharedViewingPublicKey,
      }),
      /several stored key records/,
    );
  });

  it("the current record matches the signer set and the registered key", async () => {
    const settings = await settingsAddress();
    const [a, b, c] = [member(), member(), member()];
    const store = new InMemoryKeyRecordStore();
    const first = recordFor(settings, [a, b], 0n);
    const second = recordFor(settings, [a, b, c], 1n);
    // An upload whose rotate never landed: same signers as `second`, other key.
    const stale = recordFor(settings, [a, b, c], 1n);
    await store.put(first.record);
    await store.put(second.record);
    await store.put(stale.record);

    const signers = [a, b, c].map(({ address }) => address);
    const current = await currentKeyRecord(store, {
      settings,
      signers,
      registeredViewingPublicKey: second.record.sharedViewingPublicKey,
    });
    assert.deepEqual(current, second.record);

    const beforeRotate = await currentKeyRecord(store, {
      settings,
      signers: [a, b].map(({ address }) => address),
      registeredViewingPublicKey: first.record.sharedViewingPublicKey,
    });
    assert.deepEqual(beforeRotate, first.record);

    await assert.rejects(
      currentKeyRecord(store, {
        settings,
        signers: [a].map(({ address }) => address),
      }),
      /no stored key record/,
    );
  });
});

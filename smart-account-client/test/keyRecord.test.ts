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
  checkKeyHistory,
  compareAddresses,
  decodeKeyRecord,
  encodeKeyRecord,
  generateNullifierKey,
  recoverSharedSecrets,
  recoverOldViewingKeys,
  type KeyRecord,
} from "../src/keyRecord.js";
import { currentKeyRecord } from "../src/registry.js";
import { checkOwnEntry, memberFromKeypair } from "../src/smartAccount.js";

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
  const previousViewingKeys = Array.from({ length: Number(epoch) }, () =>
    ViewingKey.generate(),
  );
  const record = buildKeyRecord({
    settings,
    epoch,
    secrets: { viewingKey, nullifierKey },
    previousViewingKeys,
    recipients: members.map(({ address, keypair }) => ({
      key: address,
      viewingPublicKey: keypair.viewingPublicKey(),
    })),
  });
  return { record, viewingKey, nullifierKey, previousViewingKeys };
}

function rotateRecord(
  previous: ReturnType<typeof recordFor>,
  members: readonly Member[],
) {
  const viewingKey = ViewingKey.generate();
  const previousViewingKeys = [
    ...previous.previousViewingKeys,
    previous.viewingKey,
  ];
  const record = buildKeyRecord({
    settings: previous.record.settings,
    epoch: previous.record.epoch + 1n,
    secrets: { viewingKey, nullifierKey: previous.nullifierKey },
    previousViewingKeys,
    recipients: members.map(({ address, keypair }) => ({
      key: address,
      viewingPublicKey: keypair.viewingPublicKey(),
    })),
  });
  return {
    record,
    viewingKey,
    nullifierKey: previous.nullifierKey,
    previousViewingKeys,
  };
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

  it("round-trips the signer entries and complete historical key vector", async () => {
    const members = [member(), member()];
    const { record } = recordFor(await settingsAddress(), members, 7n);
    const encoded = encodeKeyRecord(record);
    assert.equal(encoded.length, 177 + 97 * 2 + 65 * 7);
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

  it("new signers recover every earlier key from the latest record alone", async () => {
    const alice = member();
    const bob = member();
    const carol = member();
    const first = recordFor(await settingsAddress(), [alice]);
    const second = rotateRecord(first, [alice, bob]);
    const third = rotateRecord(second, [alice, carol]);
    checkKeyHistory(second.record, first.record);
    checkOwnEntry(
      third.record,
      memberFromKeypair(carol.keypair),
      second.record,
    );
    const secrets = recoverSharedSecrets(
      third.record,
      carol.address,
      carol.keypair,
    );
    const old = recoverOldViewingKeys(third.record, secrets.viewingKey);
    try {
      assert.deepEqual(
        old.map((key) => key.secretBytes()),
        [first.viewingKey.secretBytes(), second.viewingKey.secretBytes()],
      );
      assert.deepEqual(
        decodeKeyRecord(encodeKeyRecord(third.record)),
        third.record,
      );
      assert.throws(
        () => recoverSharedSecrets(third.record, bob.address, bob.keypair),
        /no entry/,
      );
      assert.throws(
        () => recoverOldViewingKeys(third.record, first.viewingKey),
        /current shared viewing key/,
      );
    } finally {
      old.forEach((key) => key.destroy());
      secrets.viewingKey.destroy();
      secrets.nullifierKey.destroy();
    }
  });

  it("derives a distinct encryption key for each historical secret", async () => {
    const fixture = recordFor(await settingsAddress(), [member()], 2n);
    const streams = fixture.record.oldViewingKeys.map((entry, index) => {
      const secret = fixture.previousViewingKeys[index]!.secretBytes();
      return entry.ciphertext.map((byte, offset) => byte ^ secret[offset]!);
    });
    assert.notDeepEqual(
      streams[0],
      streams[1],
      "historical ciphertexts must not reuse the CTR keystream",
    );
    assert.throws(
      () =>
        recoverOldViewingKeys(
          {
            ...fixture.record,
            oldViewingKeys: [...fixture.record.oldViewingKeys].reverse(),
          },
          fixture.viewingKey,
        ),
      /old viewing secret does not match/,
    );
  });

  it("approvers reject corrupted historical ciphertexts", async () => {
    const alice = member();
    const first = recordFor(await settingsAddress(), [alice]);
    const second = rotateRecord(first, [alice]);
    const tampered: KeyRecord = {
      ...second.record,
      oldViewingKeys: second.record.oldViewingKeys.map((entry) => ({
        ...entry,
        ciphertext: entry.ciphertext.map(
          (byte) => byte ^ 1,
        ) as typeof entry.ciphertext,
      })),
    };
    assert.throws(
      () =>
        checkOwnEntry(tampered, memberFromKeypair(alice.keypair), first.record),
      /old viewing secret does not match/,
    );
  });

  it("approvers reject omitted or substituted old keys and require the previous record", async () => {
    const alice = member();
    const first = recordFor(await settingsAddress(), [alice]);
    const second = rotateRecord(first, [alice]);
    const third = rotateRecord(second, [alice]);
    assert.throws(
      () => checkOwnEntry(second.record, memberFromKeypair(alice.keypair)),
      /needs the previous key record/,
    );
    assert.throws(
      () =>
        checkKeyHistory(
          {
            ...third.record,
            oldViewingKeys: third.record.oldViewingKeys.slice(1),
          },
          second.record,
        ),
      /count must match/,
    );
    assert.throws(
      () =>
        checkKeyHistory(
          {
            ...third.record,
            oldViewingKeys: [...third.record.oldViewingKeys].reverse(),
          },
          second.record,
        ),
      /complete previous history/,
    );
    const substituted = buildKeyRecord({
      settings: first.record.settings,
      epoch: 1n,
      secrets: {
        viewingKey: second.viewingKey,
        nullifierKey: first.nullifierKey,
      },
      previousViewingKeys: [ViewingKey.generate()],
      recipients: [
        {
          key: alice.address,
          viewingPublicKey: alice.keypair.viewingPublicKey(),
        },
      ],
    });
    assert.throws(
      () =>
        checkOwnEntry(
          substituted,
          memberFromKeypair(alice.keypair),
          first.record,
        ),
      /complete previous history/,
    );
  });

  it("the codec rejects missing, truncated and mismatched historical key vectors", async () => {
    const fixture = recordFor(await settingsAddress(), [member()], 1n);
    const encoded = encodeKeyRecord(fixture.record);
    assert.throws(() => decodeKeyRecord(encoded.slice(0, -1)), /truncated/);
    assert.throws(() => decodeKeyRecord(encoded.slice(0, -69)), /truncated/);
    assert.throws(
      () => encodeKeyRecord({ ...fixture.record, oldViewingKeys: [] }),
      /count must match/,
    );
  });

  it("the builder rejects duplicate historical keys", async () => {
    const alice = member();
    const fixture = recordFor(await settingsAddress(), [alice]);
    const build = (previousViewingKeys: ViewingKey[], epoch: bigint) =>
      buildKeyRecord({
        settings: fixture.record.settings,
        epoch,
        secrets: {
          viewingKey: fixture.viewingKey,
          nullifierKey: fixture.nullifierKey,
        },
        previousViewingKeys,
        recipients: [
          {
            key: alice.address,
            viewingPublicKey: alice.keypair.viewingPublicKey(),
          },
        ],
      });
    assert.throws(() => build([fixture.viewingKey], 1n), /duplicate key/);
    const old = ViewingKey.generate();
    assert.throws(() => build([old, old], 2n), /duplicate key/);
    assert.throws(() => build([], 1n), /every previous epoch/);
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
      recordAddress: undefined,
      recordRevision: 0n,
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

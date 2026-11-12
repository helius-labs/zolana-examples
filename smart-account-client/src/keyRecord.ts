/**
 * The key record (design.md, "Key Record"): the shared viewing secret
 * encrypted to every signer, with the nullifier secret and all earlier viewing
 * secrets encrypted to the current shared viewing key.
 *
 * Encryption of `plaintext` to recipient public key `R` under ephemeral
 * secret `e` (public key `E`):
 *
 *   dh  = X25519(e, R)
 *   key = HKDF-SHA256(ikm = dh, salt = E || R, info) -> 32 bytes
 *   ct  = AES-256-CTR(key, iv = 0^16, plaintext)
 *
 * HKDF info separates signer, nullifier and indexed historical-key encryption,
 * so the zero IV never repeats under a derived key.
 * Nothing on chain checks a ciphertext; each signer checks its own entry.
 */
import { createCipheriv, hkdfSync, randomFillSync } from "node:crypto";

import {
  getAddressDecoder,
  getAddressEncoder,
  type Address,
} from "@solana/kit";
import {
  NullifierKey,
  ViewingKey,
  X25519Keypair,
  X25519PublicKey,
  type Bytes31,
  type Bytes32,
  type Bytes33,
} from "@heliuslabs/spr/keypair";

import { ByteReader, ByteWriter, bytesEqual } from "./bytes.js";

const KEY_INFO = new TextEncoder().encode(
  "zolana-examples/smart-account-client/key",
);
const NULLIFIER_INFO = new TextEncoder().encode(
  "zolana-examples/smart-account-client/nullifier",
);
const OLD_KEY_INFO = new TextEncoder().encode(
  "zolana-examples/smart-account-client/old-key",
);

const addressEncoder = getAddressEncoder();
const addressDecoder = getAddressDecoder();

export interface KeyEntry {
  /** The signer's Solana public key, as in Squads `Settings.signers`. */
  readonly signer: Address;
  /** The signer's tagged X25519 viewing public key. */
  readonly viewingPublicKey: Bytes33;
  /** The shared viewing secret encrypted to `viewingPublicKey`. */
  readonly ciphertext: Bytes32;
}

export interface EncryptedViewingKey {
  /** The old secret's public key, checked after decryption. */
  readonly viewingPublicKey: Bytes33;
  /** The old viewing secret encrypted to the current shared viewing key. */
  readonly ciphertext: Bytes32;
}

export interface KeyRecord {
  /** The Squads Settings account this record belongs to. */
  readonly settings: Address;
  /** 0 at create, incremented by every rotate. */
  readonly epoch: bigint;
  readonly sharedViewingPublicKey: Bytes33;
  readonly ephemeralPublicKey: Bytes33;
  /** Poseidon(nullifier secret); rotate keeps it unchanged. */
  readonly nullifierPublicKey: Bytes32;
  /** The 31-byte nullifier secret encrypted to `sharedViewingPublicKey`. */
  readonly nullifierCiphertext: Bytes31;
  /** One entry per Squads signer, sorted by signer. */
  readonly entries: readonly KeyEntry[];
  /** Every previous epoch's viewing key, encrypted, oldest first. */
  readonly oldViewingKeys: readonly EncryptedViewingKey[];
}

/** A signer as the key record needs it: its key and viewing public key. */
export interface KeyRecipient {
  readonly key: Address;
  readonly viewingPublicKey: X25519PublicKey;
}

/** The signer's view of its own viewing key: an ECDH and its public key. */
export interface SignerViewingKey {
  publicKey(): X25519PublicKey;
  ecdh(counterparty: X25519PublicKey): Uint8Array;
}

/** The vault's two shared secrets, recovered from or written to a record. */
export interface SharedSecrets {
  readonly viewingKey: ViewingKey;
  readonly nullifierKey: NullifierKey;
}

export function encodeKeyRecord(record: KeyRecord): Uint8Array {
  checkHistoryLength(record);
  const writer = new ByteWriter()
    .bytes(addressEncoder.encode(record.settings))
    .u64(record.epoch)
    .bytes(record.sharedViewingPublicKey)
    .bytes(record.ephemeralPublicKey)
    .bytes(record.nullifierPublicKey)
    .bytes(record.nullifierCiphertext)
    .u32(record.entries.length);
  for (const entry of record.entries) {
    writer
      .bytes(addressEncoder.encode(entry.signer))
      .bytes(entry.viewingPublicKey)
      .bytes(entry.ciphertext);
  }
  writer.u32(record.oldViewingKeys.length);
  for (const oldKey of record.oldViewingKeys) {
    writer.bytes(oldKey.viewingPublicKey).bytes(oldKey.ciphertext);
  }
  return writer.finish();
}

export function decodeKeyRecord(data: Uint8Array): KeyRecord {
  const reader = new ByteReader(data, "key record");
  const settings = addressDecoder.decode(reader.bytes(32, "settings"));
  const epoch = reader.u64("epoch");
  const sharedViewingPublicKey = reader.bytes(
    33,
    "shared_viewing_pubkey",
  ) as Bytes33;
  const ephemeralPublicKey = reader.bytes(33, "ephemeral_pubkey") as Bytes33;
  const nullifierPublicKey = reader.bytes(32, "nullifier_pubkey") as Bytes32;
  const nullifierCiphertext = reader.bytes(
    31,
    "nullifier_ciphertext",
  ) as Bytes31;
  const count = reader.u32("entries length");
  const entries: KeyEntry[] = [];
  for (let index = 0; index < count; index += 1) {
    entries.push({
      signer: addressDecoder.decode(reader.bytes(32, "entry signer")),
      viewingPublicKey: reader.bytes(33, "entry viewing_pubkey") as Bytes33,
      ciphertext: reader.bytes(32, "entry ciphertext") as Bytes32,
    });
  }
  const oldCount = reader.u32("old viewing keys length");
  if (BigInt(oldCount) !== epoch) {
    throw new Error("old viewing key count must match the record epoch");
  }
  const oldViewingKeys: EncryptedViewingKey[] = [];
  for (let index = 0; index < oldCount; index += 1) {
    oldViewingKeys.push({
      viewingPublicKey: reader.bytes(33, "old viewing public key") as Bytes33,
      ciphertext: reader.bytes(32, "old viewing secret ciphertext") as Bytes32,
    });
  }
  reader.end();
  return {
    settings,
    epoch,
    sharedViewingPublicKey,
    ephemeralPublicKey,
    nullifierPublicKey,
    nullifierCiphertext,
    entries,
    oldViewingKeys,
  };
}

function checkHistoryLength(record: KeyRecord): void {
  if (BigInt(record.oldViewingKeys.length) !== record.epoch) {
    throw new Error("old viewing key count must match the record epoch");
  }
}

/** Verify against the previous published record before approving a rotation. */
export function checkKeyHistory(record: KeyRecord, previous?: KeyRecord): void {
  checkHistoryLength(record);
  if (previous === undefined) {
    if (record.epoch !== 0n)
      throw new Error("rotation approval needs the previous key record");
    return;
  }
  checkHistoryLength(previous);
  if (
    record.settings !== previous.settings ||
    record.epoch !== previous.epoch + 1n ||
    !bytesEqual(record.nullifierPublicKey, previous.nullifierPublicKey)
  ) {
    throw new Error("key record does not extend the previous record");
  }
  const expected = [
    ...previous.oldViewingKeys.map(({ viewingPublicKey }) => viewingPublicKey),
    previous.sharedViewingPublicKey,
  ];
  if (
    !record.oldViewingKeys.every((entry, index) => {
      const publicKey = expected.at(index);
      return (
        publicKey !== undefined && bytesEqual(entry.viewingPublicKey, publicKey)
      );
    })
  ) {
    throw new Error(
      "old viewing keys must preserve the complete previous history",
    );
  }
}

function oldKeyInfo(index: number): Uint8Array {
  return new ByteWriter().bytes(OLD_KEY_INFO).u32(index).finish();
}

function encryptOldViewingKeys(
  ephemeral: X25519Keypair,
  current: X25519PublicKey,
  previous: readonly ViewingKey[],
): EncryptedViewingKey[] {
  if (previous.length === 0) return [];
  const dh = ephemeral.ecdh(current);
  const ephemeralPublicKey = ephemeral.publicKey().toBytes();
  const currentPublicKey = current.toBytes();
  try {
    return previous.map((key, index) => {
      const secret = key.secretBytes();
      try {
        return {
          viewingPublicKey: key.publicKey().toBytes(),
          ciphertext: ctrApply(
            streamKey(
              dh,
              ephemeralPublicKey,
              currentPublicKey,
              oldKeyInfo(index),
            ),
            secret,
          ) as Bytes32,
        };
      } finally {
        secret.fill(0);
      }
    });
  } finally {
    dh.fill(0);
  }
}

function streamKey(
  dh: Uint8Array,
  ephemeralPublicKey: Uint8Array,
  recipientPublicKey: Uint8Array,
  info: Uint8Array,
): Buffer {
  const salt = new Uint8Array(
    ephemeralPublicKey.length + recipientPublicKey.length,
  );
  salt.set(ephemeralPublicKey, 0);
  salt.set(recipientPublicKey, ephemeralPublicKey.length);
  return Buffer.from(hkdfSync("sha256", dh, salt, info, 32));
}

/**
 * AES-256-CTR with a zero IV; encryption and decryption are the same. Zeroes
 * the key and the intermediate buffers; the caller owns the returned copy.
 */
function ctrApply(key: Buffer, data: Uint8Array): Uint8Array {
  const cipher = createCipheriv("aes-256-ctr", key, Buffer.alloc(16));
  const parts = [cipher.update(data), cipher.final()];
  const joined = Buffer.concat(parts);
  try {
    return new Uint8Array(joined);
  } finally {
    key.fill(0);
    joined.fill(0);
    parts.forEach((part) => part.fill(0));
  }
}

function encryptTo(
  ephemeral: X25519Keypair,
  recipient: X25519PublicKey,
  plaintext: Uint8Array,
  info: Uint8Array,
): Uint8Array {
  const dh = ephemeral.ecdh(recipient);
  try {
    return ctrApply(
      streamKey(dh, ephemeral.publicKey().toBytes(), recipient.toBytes(), info),
      plaintext,
    );
  } finally {
    dh.fill(0);
  }
}

function decryptWith(
  key: SignerViewingKey,
  ephemeralPublicKey: Bytes33,
  ciphertext: Uint8Array,
  info: Uint8Array,
): Uint8Array {
  const dh = key.ecdh(X25519PublicKey.fromBytes(ephemeralPublicKey));
  try {
    return ctrApply(
      streamKey(dh, ephemeralPublicKey, key.publicKey().toBytes(), info),
      ciphertext,
    );
  } finally {
    dh.fill(0);
  }
}

/** A fresh random nullifier secret; 31 bytes always fit the BN254 field. */
export function generateNullifierKey(): NullifierKey {
  const secret = randomFillSync(new Uint8Array(31));
  try {
    // `fromSecret` keeps its own copy.
    return NullifierKey.fromSecret(secret as Bytes31);
  } finally {
    secret.fill(0);
  }
}

/**
 * Encrypts `secrets` to `recipients` under one new ephemeral key. Create
 * passes epoch 0 and fresh secrets; rotate passes the next epoch, a new
 * viewing key, the recovered nullifier key and all prior viewing keys,
 * oldest first, ending with the previous current key.
 */
export function buildKeyRecord(
  input: Readonly<{
    settings: Address;
    epoch: bigint;
    secrets: SharedSecrets;
    previousViewingKeys: readonly ViewingKey[];
    recipients: readonly KeyRecipient[];
  }>,
): KeyRecord {
  if (BigInt(input.previousViewingKeys.length) !== input.epoch) {
    throw new Error("one old viewing key is required for every previous epoch");
  }
  const publicKeys = new Set<string>([
    Buffer.from(input.secrets.viewingKey.publicKey().toBytes()).toString("hex"),
  ]);
  for (const key of input.previousViewingKeys) {
    const publicKey = Buffer.from(key.publicKey().toBytes()).toString("hex");
    if (publicKeys.has(publicKey))
      throw new Error("viewing key history contains a duplicate key");
    publicKeys.add(publicKey);
  }
  if (input.recipients.length === 0)
    throw new Error("a key record needs a signer");
  const recipients = [...input.recipients].sort((a, b) =>
    compareAddresses(a.key, b.key),
  );
  const seen = new Set<Address>();
  for (const { key } of recipients) {
    if (seen.has(key)) throw new Error(`signer ${key} is listed twice`);
    seen.add(key);
  }

  const ephemeral = X25519Keypair.generate();
  try {
    const sharedViewingPublicKey = input.secrets.viewingKey.publicKey();
    const viewingSecret = input.secrets.viewingKey.secretBytes();
    const nullifierSecret = input.secrets.nullifierKey.secretBytes();
    try {
      return {
        settings: input.settings,
        epoch: input.epoch,
        sharedViewingPublicKey: sharedViewingPublicKey.toBytes(),
        ephemeralPublicKey: ephemeral.publicKey().toBytes(),
        nullifierPublicKey: input.secrets.nullifierKey.publicKey(),
        nullifierCiphertext: encryptTo(
          ephemeral,
          sharedViewingPublicKey,
          nullifierSecret,
          NULLIFIER_INFO,
        ) as Bytes31,
        entries: recipients.map((recipient) => ({
          signer: recipient.key,
          viewingPublicKey: recipient.viewingPublicKey.toBytes(),
          ciphertext: encryptTo(
            ephemeral,
            recipient.viewingPublicKey,
            viewingSecret,
            KEY_INFO,
          ) as Bytes32,
        })),
        oldViewingKeys: encryptOldViewingKeys(
          ephemeral,
          sharedViewingPublicKey,
          input.previousViewingKeys,
        ),
      };
    } finally {
      viewingSecret.fill(0);
      nullifierSecret.fill(0);
    }
  } finally {
    ephemeral.destroy();
  }
}

/** Every current signer can recover the old viewing secrets from this record. */
export function recoverOldViewingKeys(
  record: KeyRecord,
  current: ViewingKey,
): ViewingKey[] {
  checkHistoryLength(record);
  if (
    !bytesEqual(current.publicKey().toBytes(), record.sharedViewingPublicKey)
  ) {
    throw new Error("historical keys require the current shared viewing key");
  }
  const recovered: ViewingKey[] = [];
  if (record.oldViewingKeys.length === 0) return recovered;
  const dh = current.ecdh(X25519PublicKey.fromBytes(record.ephemeralPublicKey));
  const seen = new Set<string>([
    Buffer.from(record.sharedViewingPublicKey).toString("hex"),
  ]);
  try {
    for (const [index, entry] of record.oldViewingKeys.entries()) {
      const publicKey = Buffer.from(entry.viewingPublicKey).toString("hex");
      if (seen.has(publicKey))
        throw new Error("viewing key history contains a duplicate key");
      seen.add(publicKey);
      const secret = ctrApply(
        streamKey(
          dh,
          record.ephemeralPublicKey,
          record.sharedViewingPublicKey,
          oldKeyInfo(index),
        ),
        entry.ciphertext,
      );
      let key;
      try {
        key = ViewingKey.fromBytes(secret as Bytes32);
      } finally {
        secret.fill(0);
      }
      if (!bytesEqual(key.publicKey().toBytes(), entry.viewingPublicKey)) {
        key.destroy();
        throw new Error(
          "the decrypted old viewing secret does not match its public key",
        );
      }
      recovered.push(key);
    }
    return recovered;
  } catch (error) {
    recovered.forEach((key) => key.destroy());
    throw error;
  } finally {
    dh.fill(0);
  }
}

/**
 * Decrypts the signer's entry, then the nullifier secret, and checks both
 * against the record's public keys. A failure means the record holds a bad
 * ciphertext for this signer.
 */
export function recoverSharedSecrets(
  record: KeyRecord,
  signer: Address,
  viewing: SignerViewingKey,
): SharedSecrets {
  const entry = record.entries.find((candidate) => candidate.signer === signer);
  if (entry === undefined)
    throw new Error(`${signer} has no entry in the key record`);
  const ownPublicKey = viewing.publicKey().toBytes();
  if (!bytesEqual(entry.viewingPublicKey, ownPublicKey)) {
    throw new Error(
      `the key record entry of ${signer} is for another viewing key`,
    );
  }
  const viewingSecret = decryptWith(
    viewing,
    record.ephemeralPublicKey,
    entry.ciphertext,
    KEY_INFO,
  );
  // `fromBytes` and `fromSecret` keep their own copies, so the decrypted
  // plaintexts can be zeroed right away.
  const viewingKey = ViewingKey.fromBytes(viewingSecret as Bytes32);
  viewingSecret.fill(0);
  if (
    !bytesEqual(viewingKey.publicKey().toBytes(), record.sharedViewingPublicKey)
  ) {
    viewingKey.destroy();
    throw new Error(
      "the decrypted shared viewing secret does not match the key record",
    );
  }
  const nullifierSecret = decryptWith(
    viewingKey,
    record.ephemeralPublicKey,
    record.nullifierCiphertext,
    NULLIFIER_INFO,
  );
  const nullifierKey = NullifierKey.fromSecret(nullifierSecret as Bytes31);
  nullifierSecret.fill(0);
  if (!bytesEqual(nullifierKey.publicKey(), record.nullifierPublicKey)) {
    viewingKey.destroy();
    nullifierKey.destroy();
    throw new Error(
      "the decrypted nullifier secret does not match the key record",
    );
  }
  return { viewingKey, nullifierKey };
}

/**
 * The on-chain state a key record must match to be current: the Squads
 * signer set and the vault's registered viewing key.
 */
export interface OnChainKeyState {
  readonly settings: Address;
  readonly signers: readonly Address[];
  readonly registeredViewingPublicKey: Bytes33;
}

export function isCurrentKeyRecord(
  record: KeyRecord,
  state: OnChainKeyState,
): boolean {
  if (record.settings !== state.settings) return false;
  const recordSigners = record.entries
    .map((entry) => entry.signer)
    .sort(compareAddresses);
  const signers = [...state.signers].sort(compareAddresses);
  return (
    recordSigners.length === signers.length &&
    recordSigners.every((signer, index) => signer === signers[index]) &&
    bytesEqual(record.sharedViewingPublicKey, state.registeredViewingPublicKey)
  );
}

/** Byte order of the 32-byte keys, which is the order Squads sorts signers in. */
export function compareAddresses(a: Address, b: Address): number {
  const left = addressEncoder.encode(a);
  const right = addressEncoder.encode(b);
  for (let index = 0; index < left.length; index += 1) {
    const difference = (left[index] ?? 0) - (right[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

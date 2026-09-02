import { address } from "@solana/kit";
import { expand, extract } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import {
  encodeConfidentialSlots,
  type AssetRegistry,
  type ProofOutputUtxo,
  type WalletAuthority,
} from "@heliuslabs/zolana/transaction";
import {
  ed25519DerivationMessage,
  NullifierKey,
  randomSalt,
  ShieldedAddress,
  ShieldedPublicKey,
  ViewingKey,
  type Bytes31,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";

export type SignMessageFn = (message: Uint8Array) => Promise<Uint8Array>;

const INFO_NF = new TextEncoder().encode("TSPP/nf_key/ed25519/v1");
const INFO_VIEW = new TextEncoder().encode("TSPP/view_key/ed25519/v1");
const P256_ORDER =
  115_792_089_210_356_248_762_697_446_949_407_573_529_996_955_224_135_760_342_422_259_061_068_512_044_369n;

function bytesToBigInt(bytes: Uint8Array): bigint {
  let value = 0n;
  for (const byte of bytes) value = (value << 8n) | BigInt(byte);
  return value;
}

function scalarFromOkm(okm: Uint8Array): Bytes32 {
  const scalar = bytesToBigInt(okm) % P256_ORDER;
  if (scalar === 0n) throw new Error("viewing scalar was zero");
  const bytes = new Uint8Array(32);
  let value = scalar;
  for (let i = 31; i >= 0; i--) {
    bytes[i] = Number(value & 0xffn);
    value >>= 8n;
  }
  return bytes as Bytes32;
}

export function expandRoles(signature: Uint8Array): {
  viewing: ViewingKey;
  nullifier: NullifierKey;
} {
  const prk = extract(sha256, signature);
  const nf = expand(sha256, prk, INFO_NF, 31) as Bytes31;
  const okm = expand(sha256, prk, INFO_VIEW, 48);
  try {
    return {
      viewing: ViewingKey.fromBytes(scalarFromOkm(okm)),
      nullifier: NullifierKey.fromSecret(nf),
    };
  } finally {
    okm.fill(0);
  }
}

export class AdapterWalletAuthority implements WalletAuthority {
  readonly #solanaPublicKey: ReturnType<typeof address>;
  readonly #viewing: ViewingKey;
  readonly #nullifier: NullifierKey;
  readonly #shielded: ShieldedAddress;

  constructor(input: {
    solanaPublicKey: ReturnType<typeof address>;
    ed25519PublicKey: Bytes32;
    viewing: ViewingKey;
    nullifier: NullifierKey;
  }) {
    this.#solanaPublicKey = input.solanaPublicKey;
    this.#viewing = input.viewing;
    this.#nullifier = input.nullifier;
    this.#shielded = ShieldedAddress.fromPublicKeys(
      ShieldedPublicKey.fromEd25519(input.ed25519PublicKey),
      input.nullifier.publicKey(),
      input.viewing.publicKey(),
    );
  }

  solanaPublicKey() {
    return this.#solanaPublicKey;
  }
  shieldedAddress() {
    return Promise.resolve(this.#shielded);
  }
  viewingKeys() {
    return Promise.resolve([this.#viewing]);
  }
  spendNullifierKey() {
    return Promise.resolve(this.#nullifier);
  }
  syncMaterial() {
    return Promise.resolve({
      identity: this.#shielded,
      viewingKeys: [this.#viewing],
      nullifierKey: this.#nullifier,
    });
  }

  encryptConfidentialTransfer(input: {
    firstNullifier: Bytes32;
    outputs: readonly ProofOutputUtxo[];
    assets: AssetRegistry;
  }) {
    const tx = this.#viewing.transactionViewingKey(input.firstNullifier);
    const salt = randomSalt();
    return Promise.resolve({
      txViewingPublicKey: tx.publicKey(),
      salt,
      payload: encodeConfidentialSlots(
        input.outputs,
        input.assets,
        tx,
        salt,
      ),
    });
  }

  encryptAnonymousTransfer(): Promise<never> {
    return Promise.reject(
      new Error("this example is the confidential default ring"),
    );
  }
  encryptSplit(): Promise<never> {
    return Promise.reject(
      new Error("this example is the confidential default ring"),
    );
  }
  requestUserApproval() {
    return Promise.resolve();
  }
}

export async function deriveAdapterAuthority(input: {
  solanaPublicKey: ReturnType<typeof address>;
  ed25519PublicKey: Bytes32;
  signMessage: SignMessageFn;
}): Promise<AdapterWalletAuthority> {
  const message = ed25519DerivationMessage(input.ed25519PublicKey);
  const signature = await input.signMessage(message);
  const roles = expandRoles(signature);
  return new AdapterWalletAuthority({
    solanaPublicKey: input.solanaPublicKey,
    ed25519PublicKey: input.ed25519PublicKey,
    viewing: roles.viewing,
    nullifier: roles.nullifier,
  });
}

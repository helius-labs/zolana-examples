import { beforeAll, describe, expect, it } from "vitest";
import { getAddressDecoder } from "@solana/kit";
import { initializePoseidon } from "@heliuslabs/zolana";
import {
  ShieldedKeypair,
  SigningKey,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import { deriveAdapterAuthority } from "./deriveAuthority";

const SEED = new Uint8Array(32).fill(7) as Bytes32;

describe("deriveAdapterAuthority", () => {
  beforeAll(async () => {
    await initializePoseidon();
  });

  it("matches ShieldedKeypair.fromKeypair viewing and nullifier keys", async () => {
    const signing = SigningKey.fromEd25519Bytes(new Uint8Array(SEED) as Bytes32);
    const keypair = ShieldedKeypair.fromKeypair(signing);
    const ed25519 = keypair.signingPublicKey().ed25519();
    const solanaPublicKey = getAddressDecoder().decode(ed25519);
    const signature = signing.derivationSeed();

    const authority = await deriveAdapterAuthority({
      solanaPublicKey,
      ed25519PublicKey: ed25519,
      signMessage: async () => signature,
    });

    expect(
      Uint8Array.from((await authority.shieldedAddress()).viewingPublicKey.toBytes()),
    ).toEqual(Uint8Array.from(keypair.viewingPublicKey().toBytes()));
    expect(
      Uint8Array.from((await authority.shieldedAddress()).nullifierPublicKey),
    ).toEqual(Uint8Array.from(keypair.nullifierPublicKey()));
    expect(authority.solanaPublicKey()).toBe(solanaPublicKey);
  });
});

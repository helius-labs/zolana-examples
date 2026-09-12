import { beforeAll, describe, expect, it } from "vitest";
import { ed25519 } from "@noble/curves/ed25519.js";
import { getAddressDecoder } from "@solana/kit";
import { initializePoseidon } from "@heliuslabs/zolana";
import {
  ed25519DerivationMessage,
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

  it("signs ed25519DerivationMessage and matches ShieldedKeypair.fromKeypair", async () => {
    const secret = new Uint8Array(SEED) as Bytes32;
    const signing = SigningKey.fromEd25519Bytes(secret);
    const keypair = ShieldedKeypair.fromKeypair(signing);
    const ed25519Pk = keypair.signingPublicKey().ed25519();
    const solanaPublicKey = getAddressDecoder().decode(ed25519Pk);
    let seen: Uint8Array | undefined;

    const authority = await deriveAdapterAuthority({
      solanaPublicKey,
      ed25519PublicKey: ed25519Pk,
      signMessage: async (message) => {
        seen = message;
        return ed25519.sign(message, secret);
      },
    });

    expect(seen).toEqual(ed25519DerivationMessage(ed25519Pk));
    expect(
      Uint8Array.from((await authority.shieldedAddress()).viewingPublicKey.toBytes()),
    ).toEqual(Uint8Array.from(keypair.viewingPublicKey().toBytes()));
    expect(
      Uint8Array.from((await authority.shieldedAddress()).nullifierPublicKey),
    ).toEqual(Uint8Array.from(keypair.nullifierPublicKey()));
  });
});

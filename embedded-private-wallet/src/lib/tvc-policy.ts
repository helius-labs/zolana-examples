import type { QosIdentityPcrs } from "@zolana/tvc-wallet";
import type {
  PinnedReleaseAuthorities,
  SignedReleasePolicy,
} from "@zolana/tvc-wallet/protocol";

// Public, independently pinned pre-production trust material. The one-time
// signing key was discarded after this policy was generated; produced
// by `scripts/release.mjs policy` in zolana-tvc.
export const releasePolicy = {
  authoritySetId: "keyholder-pr9-34b4e28-2026-09",
  policy: {
    acceptedExecutableDigests: [
      "a7d194659f6ebe70df34c1e30f357a0589a1e0980193948e4f2fb430ff620571"
    ],
    acceptedManifestDigests: [
      "04399d7d0a481d912a70c6483b22b42071e2e2eb020ada5873afc2997825ca1d"
    ],
    allowedOperations: [
      "Bootstrap",
      "Decrypt",
      "Derive",
      "TransactionKeys",
      "Prove"
    ],
    environment: "development",
    expiresAtMs: "1820505815643",
    maxEncryptedRequestBytes: 262144,
    maxEncryptedResponseBytes: 262144,
    quorumKeyEpoch: "1",
    quorumKeyId: "ce7175a6-54ab-4c9b-919a-72f5548e808e",
    quorumPublicKey: "0495381809c60724e11afb56895662a6f14cea73885f3a730e30a9969c614bf97f4aaa6c93cf8f10c6078c87fa9a979dc0d76ef2ab19e7dbce0e2c776538a09c5604041a6489ee5116d149740ae75af2cd9cc5ecf85ca9487d5715243702ba95c396085e62b44c2cd2c190008c89135c104e53e86849c446eeb0f9cd3c4a0fee70b7",
    releaseId: "keyholder-pr9-34b4e28",
    revocationEpoch: "0",
    securityDomainId: "2ef3a77ae16f8443307443a475c367425394163e0106bd9218b07806cf596ff7",
    turnkeyProofSchemaVersions: [
      "turnkey.boot_proof.v1"
    ],
    turnkeyTrustRootId: "aws-nitro-root-g1",
    tvcApplicationId: "3624ddf7-61af-40e0-93b2-afc8fc766ec1",
    validFromMs: "1788969815643",
    version: 1
  },
  signatures: [
    {
      keyId: "keyholder-pr9-34b4e28-authority-1",
      scheme: "p256-sha256",
      signature: "9fb20cb17b440104c4eddbf45cb617fd282395635f28c3695067d48383d1205e7c2f8da881029f8ec08a51fe4bc937d4d2ba55af8993ab201c0a0e2fa187e693"
    }
  ]
} as const satisfies SignedReleasePolicy;

export const releaseAuthorities = {
  authoritySetId: "keyholder-pr9-34b4e28-2026-09",
  keys: [
    {
      keyId: "keyholder-pr9-34b4e28-authority-1",
      publicKey: "047a274904c18146c9504e32a07a49bd31109d095ccbf6068e6fba325ff3b255dafd71eff128fe86593e29614c338f770dbeb8073af75d204d87d23adfa8a85962"
    }
  ],
  minimumRevocationEpoch: "0",
  threshold: 1
} as const satisfies PinnedReleaseAuthorities;

export const qosIdentityPcrs = {
  0: "0f02ff921eb7189479dba97937eaa8c94e5297026d27fe47f277614d6f03d0ad9e31ddf6679a267d84730890bfefceb4",
  1: "0f02ff921eb7189479dba97937eaa8c94e5297026d27fe47f277614d6f03d0ad9e31ddf6679a267d84730890bfefceb4",
  2: "21b9efbc184807662e966d34f390821309eeac6802309798826296bf3e8bec7c10edb30948c90ba67310f7b964fc500a",
  3: "321c3cd57bd9dc5549f349c315b93167fca1adbaf19fbb9c548101bae757970fe269e530ba684826e2f5fb043319a20f"
} as const satisfies QosIdentityPcrs;

export type PrivacyWalletTrustMaterial = {
  releasePolicy: SignedReleasePolicy;
  releaseAuthorities: PinnedReleaseAuthorities;
  qosIdentityPcrs: QosIdentityPcrs;
  /**
   * The enclave's signing subkey, compressed. Derived from the pinned quorum
   * key rather than listed separately, so a key rotation cannot leave a stale
   * constant behind that still looks authoritative.
   */
  turnkeyServicePublicKey: string;
};

function compressQosSigningPublicKey(qosPublicKey: string): string {
  if (
    qosPublicKey.length !== 260 ||
    !/^04[0-9a-f]{128}04[0-9a-f]{128}$/.test(qosPublicKey)
  ) {
    throw new Error("Invalid pinned QOS public key");
  }
  const signingPublicKey = qosPublicKey.slice(130);
  const x = signingPublicKey.slice(2, 66);
  const yLastByte = Number.parseInt(signingPublicKey.slice(-2), 16);
  return `${yLastByte % 2 === 0 ? "02" : "03"}${x}`;
}

/**
 * Returns the independently pinned trust material for the deployed release.
 */
export function privacyWalletTrustMaterial(): PrivacyWalletTrustMaterial {
  return {
    releasePolicy,
    releaseAuthorities,
    qosIdentityPcrs,
    turnkeyServicePublicKey: compressQosSigningPublicKey(
      releasePolicy.policy.quorumPublicKey,
    ),
  };
}

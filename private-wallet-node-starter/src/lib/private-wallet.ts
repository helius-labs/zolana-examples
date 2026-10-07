import { webcrypto } from "node:crypto";
import {
  SOL_MINT,
  Wallet,
  createZolanaClient,
  initializePoseidon,
  syncWallet,
} from "@heliuslabs/zolana";
import {
  assertIsFullySignedTransaction,
  assertIsTransactionWithinSizeLimit,
  getSignatureFromTransaction,
  sendTransactionWithoutConfirmingFactory,
  signTransactionWithSigners,
  type Signature,
  type Transaction,
} from "@solana/kit";
import { generateP256KeyPair } from "@turnkey/crypto";
import {
  TvcKeys,
  createTvcClient,
  createTvcOperationAuthorizer,
  identityOf,
  sealedSeedOf,
  shieldedAddressOf,
  type QosIdentityPcrs,
  type TvcClientConfig,
} from "@zolana/tvc-wallet";
import {
  clientKeyIdFor,
  encodeLowerHex,
  walletEnrollmentMessage,
  type PinnedReleaseAuthorities,
  type SignedReleasePolicy,
  type WalletDescriptor,
} from "@zolana/tvc-wallet/protocol";
import { bootstrapWithApproval } from "./bootstrap-approval.js";
import {
  ENROLLMENT_DOMAIN,
  PRIVATE_WALLET_API_URL,
  SOLANA_RPC_URL,
  ZOLANA_INDEXER_URL,
  ZOLANA_PROVER_URL,
} from "./config.js";
import releaseJson from "./release.json" with { type: "json" };
import { rememberSlot, saveWallet, type StoredWallet } from "./store.js";
import {
  createSubOrganization,
  grantEnclaveBootstrap,
  signMessage,
  transactionSigner,
  turnkey,
} from "./turnkey.js";

/**
 * The enclave release this server trusts, from
 * helius-labs/zolana-tvc `apps/privacy-wallet/deploy/privacy-wallet.trust.json`.
 * The client verifies every enclave answer against it.
 */
const release = releaseJson as unknown as {
  releasePolicy: SignedReleasePolicy;
  releaseAuthorities: PinnedReleaseAuthorities;
  qosIdentityPcrs: QosIdentityPcrs;
};

/** The enclave's Turnkey key: the second point of the quorum key, compressed. */
const ENCLAVE_PUBLIC_KEY = (() => {
  const signing = release.releasePolicy.policy.quorumPublicKey.slice(130);
  const yIsEven = Number.parseInt(signing.slice(-2), 16) % 2 === 0;
  return `${yIsEven ? "02" : "03"}${signing.slice(2, 66)}`;
})();

const P256 = { name: "ECDSA", namedCurve: "P-256" } as const;

export const zolana = initializePoseidon().then(() =>
  createZolanaClient({
    solanaRpcUrl: SOLANA_RPC_URL,
    indexerUrl: ZOLANA_INDEXER_URL,
    proverUrl: ZOLANA_PROVER_URL,
    // The enclave completes resolved proof inputs; it has no indexed proving.
    proofDataSource: "client",
  }),
);

function apiUrl(path: string): URL {
  const url = new URL(PRIVATE_WALLET_API_URL);
  url.pathname += `/${path}`;
  return url;
}

/** Signs this server's enclave operations with `clientKey`. */
async function authorizer(clientKey: webcrypto.JsonWebKey) {
  const privateKey = await webcrypto.subtle.importKey(
    "jwk",
    clientKey,
    P256,
    false,
    ["sign"],
  );
  return createTvcOperationAuthorizer({
    clientKeyId: clientKeyIdFor(await publicKey(clientKey)),
    sign: async (message) =>
      new Uint8Array(
        await webcrypto.subtle.sign(
          { name: "ECDSA", hash: "SHA-256" },
          privateKey,
          message,
        ),
      ),
  });
}

/**
 * A verified connection to the enclave through the private-wallet API. With
 * a descriptor and its client key, the client can also run operations.
 */
async function enclave(
  descriptor?: WalletDescriptor,
  clientKey?: webcrypto.JsonWebKey,
) {
  const operations: TvcClientConfig["operations"] =
    descriptor && clientKey
      ? {
          walletDescriptor: descriptor,
          authorizer: await authorizer(clientKey),
        }
      : undefined;
  const client = createTvcClient({
    backend: { kind: "gateway", endpoint: new URL(PRIVATE_WALLET_API_URL) },
    ...release,
    ...(operations ? { operations } : {}),
  });
  return { client, connection: await client.connectAndVerify() };
}

async function publicKey(clientKey: webcrypto.JsonWebKey): Promise<Uint8Array> {
  const { kty, crv, x, y } = clientKey;
  const key = await webcrypto.subtle.importKey(
    "jwk",
    { kty, crv, x, y },
    P256,
    true,
    ["verify"],
  );
  return new Uint8Array(await webcrypto.subtle.exportKey("raw", key));
}

/**
 * Creates a wallet: a Turnkey wallet in your Helius project, and the private
 * keys the Helius enclave derives from it and holds.
 */
export async function createPrivateWallet(): Promise<StoredWallet> {
  const ownerKey = generateP256KeyPair();
  const owner = {
    publicKey: ownerKey.publicKey,
    privateKey: ownerKey.privateKey,
  };
  const { organizationId, walletId, address } =
    await createSubOrganization(owner);
  try {
    return await enablePrivateWallet(owner, organizationId, walletId, address);
  } catch (error) {
    console.error(
      `wallet ${address} in sub-organization ${organizationId} was created, but enabling its private wallet failed`,
    );
    throw error;
  }
}

async function enablePrivateWallet(
  owner: StoredWallet["ownerKey"],
  organizationId: string,
  walletId: string,
  address: string,
): Promise<StoredWallet> {
  const api = turnkey(owner, organizationId);

  // Verify the enclave before giving its key any authority over the wallet.
  await enclave();
  await grantEnclaveBootstrap(api, organizationId, ENCLAVE_PUBLIC_KEY, address);

  // Enroll this server's client key, with the owner's signature.
  const pair = await webcrypto.subtle.generateKey(P256, true, ["sign"]);
  const clientKey = await webcrypto.subtle.exportKey("jwk", pair.privateKey);
  const enrollment = {
    organizationId,
    turnkeyWalletId: walletId,
    solanaAddress: address,
    clientPublicKey: encodeLowerHex(await publicKey(clientKey)),
    issuedAtMs: Date.now(),
  };
  const ownerSignature = await signMessage(
    api,
    address,
    walletEnrollmentMessage({ domain: ENROLLMENT_DOMAIN, ...enrollment }),
  );
  const enrolled = await fetch(apiUrl("enroll"), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ ...enrollment, ownerSignature }),
  });
  if (!enrolled.ok) {
    throw new Error(`enroll: ${enrolled.status} ${await enrolled.text()}`);
  }
  const { descriptor } = (await enrolled.json()) as {
    descriptor: WalletDescriptor;
  };

  // The enclave derives the wallet's private keys from one owner signature.
  const { client, connection } = await enclave(descriptor, clientKey);
  const bootstrap = await bootstrapWithApproval(
    api,
    {
      organizationId,
      walletAddress: address,
      servicePublicKey: ENCLAVE_PUBLIC_KEY,
    },
    (signal) => client.bootstrap(connection, { signal }),
  );

  const wallet: StoredWallet = {
    address,
    organizationId,
    walletId,
    ownerKey: owner,
    clientKey,
    descriptor,
    identity: identityOf(bootstrap),
    sealedSeed: sealedSeedOf(bootstrap),
  };
  await saveWallet(wallet);
  return wallet;
}

export type OpenWallet = Awaited<ReturnType<typeof openPrivateWallet>>;

/**
 * A stored wallet with its enclave-held keys and Turnkey signer, synced to
 * the slot of its last transaction.
 */
export async function openPrivateWallet(stored: StoredWallet) {
  const client = await zolana;
  const { client: tvc, connection } = await enclave(
    stored.descriptor,
    stored.clientKey,
  );
  const keys = new TvcKeys({
    client: tvc,
    connection,
    identity: stored.identity,
    sealedSeed: stored.sealedSeed,
  });
  const shieldedAddress = shieldedAddressOf(stored.identity);
  const wallet = new Wallet({ identity: shieldedAddress });
  await syncWallet({
    client,
    wallet,
    keys,
    ...(stored.lastSlot
      ? { config: { requireSlot: BigInt(stored.lastSlot) } }
      : {}),
  });
  const signer = transactionSigner(
    turnkey(stored.ownerKey, stored.organizationId),
    stored.address,
  );

  /** Signs `transaction` with the Turnkey wallet, sends it and waits for it to land. */
  async function send(transaction: Transaction): Promise<Signature> {
    const signed = await signTransactionWithSigners([signer], transaction);
    assertIsFullySignedTransaction(signed);
    assertIsTransactionWithinSizeLimit(signed);
    await sendTransactionWithoutConfirmingFactory({ rpc: client.solanaRpc })(
      signed,
      { commitment: client.commitment },
    );
    const signature = getSignatureFromTransaction(signed);
    const slot = await client.confirmTransaction(signature);
    await rememberSlot(stored.address, slot);
    return signature;
  }

  return {
    keys,
    wallet,
    shieldedAddress,
    signer,
    send,
    privateLamports: () => wallet.balance(SOL_MINT).amount,
  };
}

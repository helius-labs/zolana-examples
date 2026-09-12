import {
  createTvcClient,
  identityOf,
  sealedSeedOf,
  TvcKeys,
  type TvcClientConfig,
} from "@zolana/tvc-wallet";
import {
  loadOrCreatePersistentBrowserTvcAuthorizer,
  parsePersistentBrowserTvcWalletState,
} from "@zolana/tvc-wallet/browser";
import type {
  HeliusWalletSession,
  HeliusWalletSessionWallet,
} from "helius-wallet-kit";
import { privacyWalletTrustMaterial } from "./tvc-policy";
import {
  loadKnownIdentity,
  loadWalletState,
  sameIdentity,
  saveWalletState,
  validateBinding,
  walletStorageName,
} from "./tvc-storage";

export type TvcStage = "verifying" | "enrolling" | "signing";
export type OpenTvcWalletInput = {
  wallet: HeliusWalletSessionWallet;
  parentOrganizationId: string;
  authorize: HeliusWalletSession["authorizeTvcWallet"];
  signEnrollment: HeliusWalletSession["signTvcEnrollmentChallenge"];
  withApproval: <T>(
    owner: string,
    start: (signal: AbortSignal) => Promise<T>,
    signal?: AbortSignal,
  ) => Promise<T>;
  signal: AbortSignal;
  onStage: (stage: TvcStage) => void;
};

export async function openTvcWallet(input: OpenTvcWalletInput) {
  const { wallet, signal } = input;
  const assertActive = () => signal.throwIfAborted();
  const trust = privacyWalletTrustMaterial();
  async function request(path: string, init: RequestInit = {}) {
    assertActive();
    const response = await fetch(path, {
      ...init,
      signal: AbortSignal.any([
        signal,
        AbortSignal.timeout(90_000),
        ...(init.signal ? [init.signal] : []),
      ]),
    });
    assertActive();
    if (!response.ok) {
      const failure = `TVC ${path.split("/").at(-1)} failed (HTTP ${response.status}).`;
      // Surface only recognized public error codes, never an arbitrary service body.
      const body: unknown = await response.json().catch(() => null);
      assertActive();
      const code =
        body && typeof body === "object" && "error" in body ? body.error : null;
      if (code === "CrossOriginRequestDenied")
        throw new Error(
          `${failure} The TVC backend rejected this site's origin (CrossOriginRequestDenied). Its backend or proxy origin configuration must be corrected before activation can continue.`,
        );
      if (code === "LocalProxyRequestDenied")
        throw new Error(
          `${failure} The local proxy rejected this request. Open the app directly at http://127.0.0.1:5173/ and retry.`,
        );
      throw new Error(failure);
    }
    return response;
  }
  async function post(path: string, body: unknown): Promise<unknown> {
    const response = await request(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    const result: unknown = await response.json();
    assertActive();
    return result;
  }
  const config: TvcClientConfig = {
    endpoint: new URL("https://same-origin.invalid/api/tvc/"),
    releasePolicy: trust.releasePolicy,
    releaseAuthorities: trust.releaseAuthorities,
    qosIdentityPcrs: trust.qosIdentityPcrs,
    transport: {
      fetch: (url, init) => request(`${url.pathname}${url.search}`, init),
    },
    resolveBootProof: async ({ bootProofLookupKey }) =>
      post("/api/tvc/boot-proof", {
        ephemeralKey: bootProofLookupKey,
      }) as Promise<
        Awaited<ReturnType<NonNullable<TvcClientConfig["resolveBootProof"]>>>
      >,
  };
  input.onStage("verifying");
  await createTvcClient(config).connectAndVerify();
  assertActive();

  const database = walletStorageName(input.parentOrganizationId, wallet);
  const authorizer = await loadOrCreatePersistentBrowserTvcAuthorizer({
    databaseName: `${database}:authorizer`,
  });
  assertActive();
  let state = await loadWalletState(database);
  assertActive();
  const known = await loadKnownIdentity(database);
  assertActive();
  if (known && known.solanaAddress !== wallet.address)
    throw new Error("ShieldedIdentityChanged");
  if (state) validateBinding(state, wallet, authorizer);
  if (known && state?.identity && !sameIdentity(known, state.identity))
    throw new Error("ShieldedIdentityChanged");

  input.onStage("enrolling");
  // Reconcile owner + enclave approval policy even when restoring a sealed seed.
  await input.authorize({ servicePublicKey: trust.turnkeyServicePublicKey });
  assertActive();
  if (!state) {
    const challenge = (await post("/api/tvc/enrollment-challenge", {
      parentOrganizationId: input.parentOrganizationId,
      organizationId: wallet.organizationId,
      walletName: wallet.walletName,
      turnkeyWalletId: wallet.walletId,
      solanaAddress: wallet.address,
      clientPublicKey: authorizer.clientPublicKey,
    })) as { token?: unknown; message?: unknown } | null;
    if (
      !challenge ||
      typeof challenge.token !== "string" ||
      typeof challenge.message !== "string"
    )
      throw new Error("InvalidEnrollmentChallenge");
    assertActive();
    const ownerSignature = await input.signEnrollment(challenge.message);
    assertActive();
    const descriptor = await post("/api/tvc/provision-descriptor", {
      token: challenge.token,
      ownerSignature,
    });
    state = parsePersistentBrowserTvcWalletState({
      version: 6,
      clientKeyId: authorizer.clientKeyId,
      walletDescriptor: descriptor,
      identity: null,
      sealedSeed: null,
      registered: false,
    });
    if (!state) throw new Error("InvalidWalletDescriptor");
    validateBinding(state, wallet, authorizer);
    await saveWalletState(database, state, assertActive);
  }
  const client = createTvcClient({
    ...config,
    operations: {
      walletDescriptor: state.walletDescriptor,
      authorizer: authorizer.authorizer,
    },
  });
  const connection = await client.connectAndVerify();
  assertActive();
  if (!state.identity || !state.sealedSeed) {
    input.onStage("signing");
    const result = await input.withApproval(
      wallet.address,
      (approvalSignal) =>
        client.bootstrap(connection, {
          ...(known ? { expectedIdentity: known } : {}),
          signal: approvalSignal,
        }),
      signal,
    );
    assertActive();
    state = {
      ...state,
      identity: identityOf(result),
      sealedSeed: sealedSeedOf(result),
    };
    if (state.identity!.solanaAddress !== wallet.address)
      throw new Error("WalletBindingMismatch");
    await saveWalletState(database, state, assertActive);
  }
  const keys = new TvcKeys({
    client,
    connection,
    identity: state.identity!,
    sealedSeed: state.sealedSeed!,
  });
  return {
    keys,
    markRegistered: async () => {
      assertActive();
      await saveWalletState(
        database,
        { ...state!, registered: true },
        assertActive,
      );
    },
  };
}

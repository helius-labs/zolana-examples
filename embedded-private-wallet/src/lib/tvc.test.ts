import "fake-indexeddb/auto";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createTvcClient, type TvcClientConfig } from "@zolana/tvc-wallet";
import {
  loadOrCreatePersistentBrowserTvcAuthorizer,
  saveRecord,
} from "@zolana/tvc-wallet/browser";
import { openTvcWallet, type OpenTvcWalletInput } from "./tvc";
import {
  loadWalletState,
  loadKnownIdentity,
  saveWalletState,
  walletStorageName,
} from "./tvc-storage";
import { privacyWalletTrustMaterial } from "./tvc-policy";

const mocks = vi.hoisted(() => ({
  verify: vi.fn(),
  bootstrap: vi.fn(),
  configs: [] as unknown[],
}));
vi.mock("@zolana/tvc-wallet", async (original) => ({
  ...(await original<typeof import("@zolana/tvc-wallet")>()),
  createTvcClient: vi.fn((config: unknown) => {
    mocks.configs.push(config);
    return { connectAndVerify: mocks.verify, bootstrap: mocks.bootstrap };
  }),
  TvcKeys: class {
    constructor(readonly input: unknown) {}
  },
}));
let input: OpenTvcWalletInput;
let database: string;
const owner = "11111111111111111111111111111111";
const identity = {
  solanaAddress: owner,
  shieldedOwnerHash: "11".repeat(32),
  shieldedNullifierPublicKey: "22".repeat(32),
  shieldedViewingPublicKey: "02" + "33".repeat(32),
};
let fetchMock: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.clearAllMocks();
  mocks.configs.length = 0;
  input = {
    wallet: {
      address: owner,
      walletId: crypto.randomUUID(),
      walletAccountId: "account",
      walletName: "wallet",
      organizationId: "org",
    },
    parentOrganizationId: "app",
    signal: new AbortController().signal,
    authorize: vi
      .fn()
      .mockResolvedValue({ serviceUserId: "service", apiKeyId: "key" }),
    signEnrollment: vi.fn().mockResolvedValue("owner-signature"),
    withApproval: vi.fn(async (_owner, start, signal) => start(signal!)),
    onStage: vi.fn(),
  };
  database = walletStorageName("app", input.wallet);
  mocks.verify.mockResolvedValue({});
  mocks.bootstrap.mockResolvedValue({
    solana_address: owner,
    shielded_owner_hash: identity.shieldedOwnerHash,
    shielded_nullifier_public_key: identity.shieldedNullifierPublicKey,
    shielded_viewing_public_key: identity.shieldedViewingPublicKey,
    sealed_seed: "ab".repeat(32),
  });
  fetchMock = vi.fn(async (path: string, init: RequestInit) => {
    if (path.endsWith("enrollment-challenge"))
      return Response.json({ token: "token", message: "challenge" });
    if (path.endsWith("provision-descriptor")) {
      const authorizer = await loadOrCreatePersistentBrowserTvcAuthorizer({
        databaseName: `${database}:authorizer`,
      });
      const trust = privacyWalletTrustMaterial().releasePolicy.policy;
      return Response.json({
        version: 1,
        security_domain_id: trust.securityDomainId,
        environment: trust.environment,
        turnkey_organization_id: "org",
        turnkey_wallet_id: input.wallet.walletId,
        address: owner,
        allowed_clients: [
          {
            client_public_key: authorizer.clientPublicKey,
            allowed_operations: trust.allowedOperations,
          },
        ],
        provisioning_signature: "aa",
      });
    }
    throw new Error(`Unexpected path ${path} ${init.method}`);
  });
  vi.stubGlobal("fetch", fetchMock);
});
import { afterEach } from "vitest";
afterEach(() => vi.unstubAllGlobals());
describe("TVC enrollment and recovery", () => {
  it("enrolls once, saves only sealed seed and public identity, and reloads without bootstrap", async () => {
    await openTvcWallet(input);
    const state = await loadWalletState(database);
    expect(state?.identity).toEqual(identity);
    expect(await loadKnownIdentity(database)).toEqual(identity);
    expect(Object.keys(state!).sort()).toEqual([
      "clientKeyId",
      "identity",
      "registered",
      "sealedSeed",
      "version",
      "walletDescriptor",
    ]);
    expect(input.signEnrollment).toHaveBeenCalledTimes(1);
    expect(input.withApproval).toHaveBeenCalledTimes(1);
    await openTvcWallet(input);
    expect(input.authorize).toHaveBeenCalledTimes(2); // grants reconciled on every explicit session activation
    expect(input.signEnrollment).toHaveBeenCalledTimes(1);
    expect(mocks.bootstrap).toHaveBeenCalledTimes(1);
  });
  it("fails attestation before any enrollment or signing", async () => {
    mocks.verify.mockRejectedValue(new Error("BadAttestation"));
    await expect(openTvcWallet(input)).rejects.toThrow("BadAttestation");
    expect(input.authorize).not.toHaveBeenCalled();
    expect(fetchMock).not.toHaveBeenCalled();
  });
  it("does not reset corrupt storage", async () => {
    await saveRecord(database, "wallet", { broken: true });
    await expect(openTvcWallet(input)).rejects.toThrow("StorageCorrupted");
    expect(input.authorize).not.toHaveBeenCalled();
    await expect(loadWalletState(database)).rejects.toThrow("StorageCorrupted");
  });
  it("pins bootstrap to the previously known identity", async () => {
    await saveRecord(database, "identity", identity);
    await openTvcWallet(input);
    expect(mocks.bootstrap).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ expectedIdentity: identity }),
    );
  });
  it("rejects mismatched cached identity without signing", async () => {
    await openTvcWallet(input);
    vi.mocked(input.signEnrollment).mockClear();
    await saveRecord(database, "identity", {
      ...identity,
      shieldedOwnerHash: "aa".repeat(32),
    });
    await expect(openTvcWallet(input)).rejects.toThrow(
      "ShieldedIdentityChanged",
    );
    expect(input.signEnrollment).not.toHaveBeenCalled();
  });
  it("preserves known identity if an attempted save conflicts", async () => {
    await openTvcWallet(input);
    const state = (await loadWalletState(database))!;
    await expect(
      saveWalletState(
        database,
        {
          ...state,
          identity: { ...identity, shieldedOwnerHash: "ff".repeat(32) },
        },
        () => {},
      ),
    ).rejects.toThrow("ShieldedIdentityChanged");
    expect(await loadKnownIdentity(database)).toEqual(identity);
  });
  it("stops after account changes during policy authorization", async () => {
    const controller = new AbortController();
    input.signal = controller.signal;
    input.authorize = vi.fn(async () => {
      controller.abort(new Error("Account changed"));
      return { serviceUserId: "service", apiKeyId: "key" };
    });
    await expect(openTvcWallet(input)).rejects.toThrow("Account changed");
    expect(input.signEnrollment).not.toHaveBeenCalled();
  });
  it("reports enrollment HTTP errors and never invokes bootstrap", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
    await expect(openTvcWallet(input)).rejects.toThrow(
      "enrollment-challenge failed (HTTP 403)",
    );
    expect(mocks.bootstrap).not.toHaveBeenCalled();
  });
  it("identifies a backend boot-proof origin denial before authorizing or enrolling", async () => {
    mocks.verify.mockImplementationOnce(async () => {
      const config = mocks.configs[0] as TvcClientConfig;
      await config.resolveBootProof!({
        bootProofLookupKey: "public-key",
      } as Parameters<NonNullable<TvcClientConfig["resolveBootProof"]>>[0]);
    });
    fetchMock.mockResolvedValueOnce(
      Response.json({ error: "CrossOriginRequestDenied" }, { status: 403 }),
    );
    await expect(openTvcWallet(input)).rejects.toThrow(
      "The TVC backend rejected this site's origin (CrossOriginRequestDenied)",
    );
    expect(input.authorize).not.toHaveBeenCalled();
    expect(input.signEnrollment).not.toHaveBeenCalled();
    expect(mocks.bootstrap).not.toHaveBeenCalled();
  });
  it("does not expose arbitrary upstream error details", async () => {
    fetchMock.mockResolvedValueOnce(
      Response.json({ error: "internal service detail" }, { status: 500 }),
    );
    await expect(openTvcWallet(input)).rejects.toThrow(
      /^TVC enrollment-challenge failed \(HTTP 500\)\.$/,
    );
  });
  it("uses a fixed transport with session cancellation and independent trust", async () => {
    await openTvcWallet(input);
    const config = vi.mocked(createTvcClient).mock
      .calls[0][0] as TvcClientConfig;
    expect(config.releasePolicy).toBe(
      privacyWalletTrustMaterial().releasePolicy,
    );
    expect(config.qosIdentityPcrs).toBe(
      privacyWalletTrustMaterial().qosIdentityPcrs,
    );
    expect(config.endpoint.href).toBe("https://same-origin.invalid/api/tvc/");
  });
  it("isolates different accounts and apps", () => {
    expect(walletStorageName("other-app", input.wallet)).not.toBe(database);
    expect(
      walletStorageName("app", { ...input.wallet, walletAccountId: "other" }),
    ).not.toBe(database);
  });
});

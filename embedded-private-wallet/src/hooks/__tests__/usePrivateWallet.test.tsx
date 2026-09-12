import { getPrivateSolBalance } from "../../operations/read/getBalance";
// @vitest-environment jsdom
import { StrictMode, type PropsWithChildren } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useEmbeddedWallet } from "../useEmbeddedWallet";
import { buildRegistrationTransaction } from "@heliuslabs/zolana";
import { checkRegistration } from "../../lib/registration";
import { connectClient } from "../../lib/client";
import { openTvcWallet } from "../../lib/tvc";
import { usePrivateWallet } from "../usePrivateWallet";
vi.mock("../useEmbeddedWallet", () => ({ useEmbeddedWallet: vi.fn() }));
vi.mock("../useBootstrapApproval", () => ({
  useBootstrapApproval: () => vi.fn(),
}));
vi.mock("../../operations/read/getBalance", () => ({
  getPrivateSolBalance: vi.fn(),
}));
vi.mock("../../lib/client", () => ({ connectClient: vi.fn() }));
vi.mock("../../lib/tvc", () => ({ openTvcWallet: vi.fn() }));
vi.mock("../../lib/registration", () => ({ checkRegistration: vi.fn() }));
vi.mock("@heliuslabs/zolana", () => ({
  buildRegistrationTransaction: vi.fn(),
  Wallet: class {
    balance() {
      return { amount: 0n };
    }
  },
}));
vi.mock("../../lib/turnkey-signer", () => ({
  turnkeyTransactionSigner: (
    _owner: string,
    sign: (bytes: Uint8Array) => Promise<Uint8Array>,
  ) => ({ sign }),
}));
vi.mock("../../lib/send", () => ({
  submitFactory:
    (
      _client: unknown,
      signer: { sign: (tx: Uint8Array) => Promise<Uint8Array> },
      assertActive: () => void,
    ) =>
    async (tx: Uint8Array) => {
      assertActive();
      await signer.sign(tx);
      assertActive();
      return { signature: "registration", slot: 1n };
    },
}));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
const wrapper = ({ children }: PropsWithChildren) => (
  <StrictMode>{children}</StrictMode>
);
let embedded: ReturnType<typeof useEmbeddedWallet>;
const tvc = {
  keys: { address: () => ({}) },
  markRegistered: vi.fn(),
} as unknown as Awaited<ReturnType<typeof openTvcWallet>>;
beforeEach(() => {
  vi.resetAllMocks();
  embedded = {
    connected: true,
    owner: "11111111111111111111111111111111",
    sessionKey: "session-1",
    wallet: {},
    parentOrganizationId: "app",
    signTransaction: vi.fn().mockImplementation(async (tx) => tx),
  } as unknown as typeof embedded;
  vi.mocked(useEmbeddedWallet).mockImplementation(() => embedded);
  vi.mocked(connectClient).mockResolvedValue({} as never);
  vi.mocked(openTvcWallet).mockResolvedValue(tvc);
  vi.mocked(checkRegistration).mockResolvedValue(true);
  vi.mocked(buildRegistrationTransaction).mockResolvedValue({} as never);
  vi.mocked(getPrivateSolBalance).mockResolvedValue(undefined as never);
});
afterEach(cleanup);
describe("explicit TVC activation", () => {
  it("does not enroll, bootstrap or sign on connect and Strict Mode", () => {
    const { result, rerender } = renderHook(usePrivateWallet, { wrapper });
    rerender();
    expect(result.current.status).toBe("connected");
    expect(openTvcWallet).not.toHaveBeenCalled();
    expect(embedded.signTransaction).not.toHaveBeenCalled();
  });
  it("guards duplicate activation and skips verified registration", async () => {
    const pending = deferred<typeof tvc>();
    vi.mocked(openTvcWallet).mockReturnValue(pending.promise);
    const { result } = renderHook(usePrivateWallet, { wrapper });
    let work!: Promise<void>;
    await act(async () => {
      work = result.current.initialize();
      void result.current.initialize();
    });
    expect(openTvcWallet).toHaveBeenCalledTimes(1);
    await act(async () => {
      pending.resolve(tvc);
      await work;
    });
    expect(result.current.ready).toBe(true);
    expect(result.current.ctx).not.toHaveProperty("wallet");
    expect(buildRegistrationTransaction).not.toHaveBeenCalled();
    expect(getPrivateSolBalance).toHaveBeenCalledTimes(1);
  });
  it("registers a new wallet, verifies its record and syncs", async () => {
    vi.mocked(checkRegistration)
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(embedded.signTransaction).toHaveBeenCalledTimes(1);
    expect(checkRegistration).toHaveBeenCalledTimes(2);
    expect(result.current.ready).toBe(true);
  });
  it.each([
    "AttestationRejected",
    "User rejected approval",
    "StorageCorrupted",
    "Service unavailable",
  ])("allows explicit retry after %s", async (message) => {
    vi.mocked(openTvcWallet).mockRejectedValueOnce(new Error(message));
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.error).toContain(message);
    expect(result.current.ctx).toBeNull();
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.ready).toBe(true);
  });
  it("does not overwrite a conflicting registered identity", async () => {
    vi.mocked(checkRegistration).mockRejectedValue(
      new Error("Registry identity mismatch"),
    );
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.error).toContain("mismatch");
    expect(embedded.signTransaction).not.toHaveBeenCalled();
  });
  it("reports an unavailable session before starting TVC", async () => {
    embedded.wallet = null;
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.error).toContain("Turnkey wallet is unavailable");
    expect(openTvcWallet).not.toHaveBeenCalled();
  });
  it("aborts stale bootstrap and discards its context on account change", async () => {
    const pending = deferred<typeof tvc>();
    vi.mocked(openTvcWallet).mockReturnValue(pending.promise);
    const { result, rerender } = renderHook(usePrivateWallet);
    let work!: Promise<void>;
    await act(async () => {
      work = result.current.initialize();
    });
    const signal = vi.mocked(openTvcWallet).mock.calls[0][0].signal;
    embedded = { ...embedded, sessionKey: "session-2" };
    rerender();
    expect(signal.aborted).toBe(true);
    await act(async () => {
      pending.resolve(tvc);
      await work;
    });
    expect(result.current.ctx).toBeNull();
    expect(checkRegistration).not.toHaveBeenCalled();
  });
  it("stops if disconnected during client setup", async () => {
    const pending = deferred<Awaited<ReturnType<typeof connectClient>>>();
    vi.mocked(connectClient).mockReturnValue(pending.promise);
    const { result, rerender } = renderHook(usePrivateWallet);
    let work!: Promise<void>;
    await act(async () => {
      work = result.current.initialize();
    });
    embedded = { ...embedded, connected: false, owner: "" };
    rerender();
    await act(async () => {
      pending.resolve({} as never);
      await work;
    });
    expect(openTvcWallet).not.toHaveBeenCalled();
    expect(result.current.status).toBe("disconnected");
  });
  it("rejects signing and sync continuation after session expiry", async () => {
    vi.mocked(checkRegistration).mockResolvedValueOnce(false);
    const pending = deferred<never>();
    vi.mocked(embedded.signTransaction).mockReturnValue(pending.promise);
    const { result, rerender } = renderHook(usePrivateWallet);
    let work!: Promise<void>;
    await act(async () => {
      work = result.current.initialize();
    });
    embedded = { ...embedded, sessionKey: "expired" };
    rerender();
    await act(async () => {
      pending.resolve({} as never);
      await work;
    });
    expect(getPrivateSolBalance).not.toHaveBeenCalled();
    expect(result.current.ready).toBe(false);
  });
  it("invalidates a context after unmount", async () => {
    const { result, unmount } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    const ctx = result.current.ctx!;
    unmount();
    await expect(ctx.submit({} as never)).rejects.toThrow("session changed");
  });
});

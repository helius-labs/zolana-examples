// @vitest-environment jsdom
import { StrictMode, type PropsWithChildren } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useEmbeddedWallet } from "../useEmbeddedWallet";
import { buildRegistrationTransaction, syncWallet } from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";
import { connectClient } from "../../lib/client";
import { usePrivateWallet } from "../usePrivateWallet";

vi.mock("../useEmbeddedWallet", () => ({ useEmbeddedWallet: vi.fn() }));
vi.mock("../../lib/client", () => ({ connectClient: vi.fn() }));
vi.mock("@heliuslabs/zolana", () => ({
  buildRegistrationTransaction: vi.fn(),
  syncWallet: vi.fn(),
  Wallet: class {
    balance() {
      return { amount: 0n };
    }
  },
}));
vi.mock("@heliuslabs/zolana/wallet", () => ({ isWalletRegistered: vi.fn() }));
vi.mock("../../lib/deriveAuthority", () => ({
  deriveAdapterAuthority: async ({
    signMessage,
  }: {
    signMessage: (message: Uint8Array) => Promise<Uint8Array>;
  }) => {
    await signMessage(new Uint8Array([255, 1]));
    return { shieldedAddress: async () => ({}) };
  },
}));
vi.mock("../../lib/walletAdapterSigner", () => ({
  walletAdapterSigner: (input: unknown) => input,
}));
vi.mock("../../lib/send", () => ({
  submitFactory:
    (
      _client: unknown,
      signer: { signTransaction: (tx: unknown) => Promise<unknown> },
      assertActive: () => void,
    ) =>
    async (tx: unknown) => {
      assertActive();
      await signer.signTransaction(tx);
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
let wallet: ReturnType<typeof useEmbeddedWallet>;

beforeEach(() => {
  vi.clearAllMocks();
  wallet = {
    connected: true,
    owner: "11111111111111111111111111111111",
    sessionKey: "user-1:wallet-1",
    signMessage: vi.fn().mockResolvedValue(new Uint8Array(64)),
    signTransaction: vi.fn().mockImplementation(async (tx) => tx),
  } as unknown as ReturnType<typeof useEmbeddedWallet>;
  vi.mocked(useEmbeddedWallet).mockImplementation(() => wallet);
  vi.mocked(connectClient).mockResolvedValue(
    {} as Awaited<ReturnType<typeof connectClient>>,
  );
  vi.mocked(isWalletRegistered).mockResolvedValue(true);
  vi.mocked(buildRegistrationTransaction).mockResolvedValue({} as never);
  vi.mocked(syncWallet).mockResolvedValue(undefined as never);
});
afterEach(cleanup);

describe("explicit private wallet initialization", () => {
  it("never initializes or signs on connect, rerender, or Strict Mode effects", () => {
    const { result, rerender } = renderHook(usePrivateWallet, { wrapper });
    rerender();
    expect(result.current.status).toBe("connected");
    expect(connectClient).not.toHaveBeenCalled();
    expect(wallet.signMessage).not.toHaveBeenCalled();
  });

  it("guards double clicks and skips registration for an existing wallet", async () => {
    const signing = deferred<Uint8Array>();
    vi.mocked(wallet.signMessage!).mockReturnValue(signing.promise);
    const { result } = renderHook(usePrivateWallet, { wrapper });
    let initialization!: Promise<void>;
    await act(async () => {
      initialization = result.current.initialize();
      void result.current.initialize();
    });
    expect(result.current.status).toBe("signing");
    expect(wallet.signMessage).toHaveBeenCalledTimes(1);
    await act(async () => {
      signing.resolve(new Uint8Array(64));
      await initialization;
    });
    expect(result.current.ready).toBe(true);
    expect(wallet.signTransaction).not.toHaveBeenCalled();
    expect(buildRegistrationTransaction).not.toHaveBeenCalled();
    expect(syncWallet).toHaveBeenCalledTimes(1);
  });

  it("registers a new wallet and syncs after signing the transaction", async () => {
    vi.mocked(isWalletRegistered).mockResolvedValue(false);
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(wallet.signTransaction).toHaveBeenCalledTimes(1);
    expect(result.current.status).toBe("ready");
    expect(syncWallet).toHaveBeenCalledTimes(1);
  });

  it("shows rejection and allows an explicit retry", async () => {
    vi.mocked(wallet.signMessage!).mockRejectedValueOnce(
      new Error("User rejected the request"),
    );
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.status).toBe("error");
    expect(result.current.error).toContain("User rejected");
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.ready).toBe(true);
    expect(result.current.error).toBeNull();
  });

  it("reports unsupported wallets before contacting the client", async () => {
    wallet.signMessage = undefined as never;
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.error).toContain(
      "Privy message and transaction signing are unavailable",
    );
    expect(connectClient).not.toHaveBeenCalled();
  });

  it("shows service failures and retries", async () => {
    vi.mocked(connectClient).mockRejectedValueOnce(
      new Error("Service unavailable"),
    );
    const { result } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.error).toBe("Service unavailable");
    expect(wallet.signMessage).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.initialize();
    });
    expect(result.current.ready).toBe(true);
  });

  it("stops after a pending message when the account changes", async () => {
    const signing = deferred<Uint8Array>();
    vi.mocked(wallet.signMessage!).mockReturnValue(signing.promise);
    const { result, rerender } = renderHook(usePrivateWallet);
    let initialization!: Promise<void>;
    await act(async () => {
      initialization = result.current.initialize();
    });
    wallet = {
      ...wallet,
      owner: "So11111111111111111111111111111111111111112",
    };
    rerender();
    const staleSignature = new Uint8Array(64).fill(9);
    await act(async () => {
      signing.resolve(staleSignature);
      await initialization;
    });
    expect(staleSignature.every((byte) => byte === 0)).toBe(true);
    expect(result.current.ctx).toBeNull();
    expect(result.current.status).toBe("connected");
    expect(isWalletRegistered).not.toHaveBeenCalled();
  });

  it("stops before signing if disconnected during client setup", async () => {
    const client = deferred<Awaited<ReturnType<typeof connectClient>>>();
    vi.mocked(connectClient).mockReturnValue(client.promise);
    const { result, rerender } = renderHook(usePrivateWallet);
    let initialization!: Promise<void>;
    await act(async () => {
      initialization = result.current.initialize();
    });
    wallet = { ...wallet, connected: false, owner: "" };
    rerender();
    await act(async () => {
      client.resolve({} as Awaited<ReturnType<typeof connectClient>>);
      await initialization;
    });
    expect(wallet.signMessage).not.toHaveBeenCalled();
    expect(result.current.status).toBe("disconnected");
  });

  it("does not continue after a registration prompt if the wallet changes", async () => {
    vi.mocked(isWalletRegistered).mockResolvedValue(false);
    const signing = deferred<never>();
    vi.mocked(wallet.signTransaction!).mockReturnValue(signing.promise);
    const { result, rerender } = renderHook(usePrivateWallet);
    let initialization!: Promise<void>;
    await act(async () => {
      initialization = result.current.initialize();
    });
    expect(result.current.status).toBe("registering");
    wallet = {
      ...wallet,
      sessionKey: "user-2:wallet-1",
    };
    rerender();
    await act(async () => {
      signing.resolve({} as never);
      await initialization;
    });
    expect(syncWallet).not.toHaveBeenCalled();
    expect(result.current.ready).toBe(false);
  });

  it("invalidates an existing context's submit function after unmount", async () => {
    const { result, unmount } = renderHook(usePrivateWallet);
    await act(async () => {
      await result.current.initialize();
    });
    const ctx = result.current.ctx!;
    unmount();
    await expect(ctx.submit({} as never)).rejects.toThrow("Wallet changed");
    expect(wallet.signTransaction).not.toHaveBeenCalled();
  });
});

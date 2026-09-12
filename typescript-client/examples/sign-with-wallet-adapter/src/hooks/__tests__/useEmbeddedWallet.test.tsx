// @vitest-environment jsdom
import { StrictMode, type PropsWithChildren } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePrivy } from "@privy-io/react-auth";
import {
  useWallets,
  useSignMessage,
  useSignTransaction,
  useCreateWallet,
} from "@privy-io/react-auth/solana";
import {
  Keypair,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { ed25519 } from "@noble/curves/ed25519.js";
import {
  ed25519DerivationMessage,
  type Bytes32,
} from "@heliuslabs/zolana/keypair";
import { useEmbeddedWallet } from "../useEmbeddedWallet";

vi.mock("@privy-io/react-auth", () => ({ usePrivy: vi.fn() }));
vi.mock("@privy-io/react-auth/solana", () => ({
  useWallets: vi.fn(),
  useSignMessage: vi.fn(),
  useSignTransaction: vi.fn(),
  useCreateWallet: vi.fn(),
}));
const seed = new Uint8Array(32).fill(7);
const keypair = Keypair.fromSeed(seed);
const embedded = {
  address: keypair.publicKey.toBase58(),
  standardWallet: { name: "Privy" },
};
const external = {
  address: "11111111111111111111111111111111",
  standardWallet: { name: "Phantom" },
};
const signMessage = vi.fn();
const signTransaction = vi.fn();
const createWallet = vi.fn();
let auth: ReturnType<typeof usePrivy>;
let wallets: ReturnType<typeof useWallets>;
const wrapper = ({ children }: PropsWithChildren) => (
  <StrictMode>{children}</StrictMode>
);

beforeEach(() => {
  vi.clearAllMocks();
  auth = {
    ready: true,
    authenticated: true,
    user: { id: "user-1" },
    login: vi.fn(),
    logout: vi.fn(),
  } as never;
  wallets = { ready: true, wallets: [external, embedded] } as never;
  vi.mocked(usePrivy).mockImplementation(() => auth);
  vi.mocked(useWallets).mockImplementation(() => wallets);
  vi.mocked(useSignMessage).mockReturnValue({ signMessage });
  vi.mocked(useSignTransaction).mockReturnValue({ signTransaction });
  vi.mocked(useCreateWallet).mockReturnValue({ createWallet });
  signMessage.mockImplementation(async ({ message }) => ({
    signature: ed25519.sign(message, seed),
  }));
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

describe("Privy embedded wallet bridge", () => {
  it("selects the embedded wallet even when an external wallet comes first, without signing on mount", () => {
    const { result, rerender } = renderHook(useEmbeddedWallet, { wrapper });
    rerender();
    expect(result.current.owner).toBe(embedded.address);
    expect(signMessage).not.toHaveBeenCalled();
    expect(signTransaction).not.toHaveBeenCalled();
    expect(createWallet).not.toHaveBeenCalled();
  });

  it("does not fall back to an external wallet", () => {
    wallets = { ready: true, wallets: [external] } as never;
    const { result } = renderHook(useEmbeddedWallet);
    expect(result.current.connected).toBe(false);
    expect(result.current.owner).toBe("");
  });

  it("forwards the exact binary derivation message and verifies its signature", async () => {
    const message = ed25519DerivationMessage(
      keypair.publicKey.toBytes() as Bytes32,
    );
    const { result } = renderHook(useEmbeddedWallet);
    let signature!: Uint8Array;
    await act(async () => {
      signature = await result.current.signMessage(message);
    });
    expect(signMessage).toHaveBeenCalledExactlyOnceWith({
      wallet: embedded,
      message,
      options: {
        uiOptions: { title: "Activate private wallet", showWalletUIs: true },
      },
    });
    expect(
      ed25519.verify(signature, message, keypair.publicKey.toBytes()),
    ).toBe(true);
    signature.fill(0);
  });

  it("rejects and clears a signature over modified message bytes", async () => {
    const signature = ed25519.sign(
      new TextEncoder().encode("different message"),
      seed,
    );
    signMessage.mockResolvedValue({ signature });
    const { result } = renderHook(useEmbeddedWallet);
    await expect(
      result.current.signMessage(new Uint8Array([255, 1])),
    ).rejects.toThrow("exact activation message");
    expect(signature.every((byte) => byte === 0)).toBe(true);
  });

  it("rejects malformed signatures without deriving keys", async () => {
    const signature = new Uint8Array(10).fill(9);
    signMessage.mockResolvedValue({ signature });
    const { result } = renderHook(useEmbeddedWallet);
    await expect(
      result.current.signMessage(new Uint8Array([255, 1])),
    ).rejects.toThrow("exact activation message");
    expect(signature.every((byte) => byte === 0)).toBe(true);
  });

  it("signs serialized transactions with the same wallet and explicit devnet chain", async () => {
    // web3.js uses Node buffers; use their Uint8Array realm in this jsdom test.
    vi.stubGlobal("Uint8Array", Object.getPrototypeOf(process.getBuiltinModule("buffer").Buffer));
    const transaction = new VersionedTransaction(
      new TransactionMessage({
        payerKey: keypair.publicKey,
        recentBlockhash: "11111111111111111111111111111111",
        instructions: [],
      }).compileToV0Message(),
    );
    const bytes = transaction.serialize();
    signTransaction.mockImplementation(async ({ transaction: input }) => {
      const signed = VersionedTransaction.deserialize(input);
      signed.sign([keypair]);
      return { signedTransaction: signed.serialize() };
    });
    const { result } = renderHook(useEmbeddedWallet);
    const signed = await result.current.signTransaction(transaction);
    expect(signTransaction).toHaveBeenCalledExactlyOnceWith({
      wallet: embedded,
      transaction: bytes,
      chain: "solana:devnet",
      options: { uiOptions: { showWalletUIs: true } },
    });
    expect(
      ed25519.verify(
        signed.signatures[0],
        signed.message.serialize(),
        keypair.publicKey.toBytes(),
      ),
    ).toBe(true);
  });

  it("invalidates the session on logout or a different Privy user", () => {
    const { result, rerender } = renderHook(useEmbeddedWallet);
    const session = result.current.sessionKey;
    auth = { ...auth, user: { id: "user-2" } as never };
    rerender();
    expect(result.current.sessionKey).not.toBe(session);
    auth = { ...auth, authenticated: false };
    rerender();
    expect(result.current.connected).toBe(false);
    expect(result.current.owner).toBe("");
  });

  it("waits for wallet readiness before allowing signing", () => {
    wallets = { ...wallets, ready: false };
    const { result } = renderHook(useEmbeddedWallet);
    expect(result.current.ready).toBe(false);
    expect(result.current.connected).toBe(false);
  });
});

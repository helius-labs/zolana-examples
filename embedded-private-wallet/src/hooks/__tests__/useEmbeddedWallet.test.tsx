// @vitest-environment jsdom
import { renderHook, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useHeliusWallet, useHeliusWalletSession } from "helius-wallet-kit";
import { useTurnkey } from "@turnkey/react-wallet-kit";
import { useEmbeddedWallet } from "../useEmbeddedWallet";
vi.mock("helius-wallet-kit", () => ({
  useHeliusWallet: vi.fn(),
  useHeliusWalletSession: vi.fn(),
}));
vi.mock("@turnkey/react-wallet-kit", () => ({ useTurnkey: vi.fn() }));
let session: ReturnType<typeof useHeliusWalletSession>;
let token = "session-a";
beforeEach(() => {
  token = "session-a";
  session = {
    status: "authenticated",
    userId: "user",
    activeWallet: {
      address: "owner",
      walletId: "wallet",
      organizationId: "org",
    },
    login: vi.fn(),
    logout: vi.fn(),
    authorizeTvcWallet: vi.fn(),
    signTvcEnrollmentChallenge: vi.fn(),
  } as unknown as typeof session;
  vi.mocked(useHeliusWalletSession).mockImplementation(() => session);
  vi.mocked(useHeliusWallet).mockReturnValue({
    signTransaction: vi.fn(),
  } as never);
  vi.mocked(useTurnkey).mockImplementation(
    () => ({ session: { token } }) as never,
  );
});
afterEach(cleanup);
it("exposes only the authenticated Turnkey wallet and makes no signing calls", () => {
  const { result } = renderHook(useEmbeddedWallet);
  expect(result.current.owner).toBe("owner");
  expect(result.current.connected).toBe(true);
  expect(session.authorizeTvcWallet).not.toHaveBeenCalled();
  expect(session.signTvcEnrollmentChallenge).not.toHaveBeenCalled();
});
it("hides a stale wallet after logout", () => {
  session.status = "unauthenticated";
  const { result } = renderHook(useEmbeddedWallet);
  expect(result.current.owner).toBe("");
  expect(result.current.connected).toBe(false);
});
it("changes the session binding when its token changes", () => {
  const { result, rerender } = renderHook(useEmbeddedWallet);
  const before = result.current.sessionKey;
  token = "session-b";
  rerender();
  expect(result.current.sessionKey).not.toBe(before);
});

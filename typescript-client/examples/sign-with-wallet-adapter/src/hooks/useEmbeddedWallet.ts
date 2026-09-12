import { useRef } from "react";
import { useHeliusWallet, useHeliusWalletSession } from "helius-wallet-kit";
import { useTurnkey } from "@turnkey/react-wallet-kit";

/** One Turnkey session supplies both TVC enrollment and Solana signing. */
export function useEmbeddedWallet() {
  const session = useHeliusWalletSession();
  const { signTransaction } = useHeliusWallet();
  const { session: turnkeySession } = useTurnkey();
  const binding = JSON.stringify([
    session.userId,
    session.activeWallet,
    turnkeySession?.token,
    session.status,
  ]);
  const bindingRef = useRef({ value: binding, id: 0 });
  if (bindingRef.current.value !== binding)
    bindingRef.current = { value: binding, id: bindingRef.current.id + 1 };
  const authenticated = session.status === "authenticated";
  const wallet = authenticated ? session.activeWallet : null;
  return {
    ready: session.status !== "loading",
    authenticated,
    connected: Boolean(wallet),
    owner: wallet?.address ?? "",
    // Token changes invalidate in-flight work even when the address stays the same.
    sessionKey: String(bindingRef.current.id),
    wallet,
    parentOrganizationId: session.parentOrganizationId,
    login: session.login,
    logout: session.logout,
    clear: session.clear,
    signTransaction,
    authorizeTvcWallet: session.authorizeTvcWallet,
    signTvcEnrollmentChallenge: session.signTvcEnrollmentChallenge,
  };
}

import { useCallback } from "react";
import { usePrivy } from "@privy-io/react-auth";
import {
  useCreateWallet,
  useSignMessage,
  useSignTransaction,
  useWallets,
} from "@privy-io/react-auth/solana";
import { VersionedTransaction } from "@solana/web3.js";
import { verifyDerivationSignature } from "../lib/privySigning";

/** Only Privy embedded wallets can supply the private-wallet derivation seed. */
export function useEmbeddedWallet() {
  const { ready: authReady, authenticated, user, login, logout } = usePrivy();
  const { ready: walletsReady, wallets } = useWallets();
  const { createWallet } = useCreateWallet();
  const { signMessage: privySignMessage } = useSignMessage();
  const { signTransaction: privySignTransaction } = useSignTransaction();
  const ready = authReady && walletsReady;
  const wallet =
    authenticated && ready
      ? wallets.find((candidate) => candidate.standardWallet.name === "Privy")
      : undefined;
  const owner = wallet?.address ?? "";
  const sessionKey = `${user?.id ?? ""}:${owner}`;

  const signMessage = useCallback(
    async (message: Uint8Array) => {
      if (!wallet)
        throw new Error("Sign in to your Privy embedded wallet first.");
      const { signature } = await privySignMessage({
        wallet,
        message,
        options: {
          uiOptions: { title: "Activate private wallet", showWalletUIs: true },
        },
      });
      // Reject altered/prefixed signing before deriving a different private identity.
      return verifyDerivationSignature(wallet.address, message, signature);
    },
    [wallet, privySignMessage],
  );

  const signTransaction = useCallback(
    async (transaction: VersionedTransaction) => {
      if (!wallet)
        throw new Error("Sign in to your Privy embedded wallet first.");
      const { signedTransaction } = await privySignTransaction({
        wallet,
        transaction: transaction.serialize(),
        chain: "solana:devnet",
        options: { uiOptions: { showWalletUIs: true } },
      });
      return VersionedTransaction.deserialize(signedTransaction);
    },
    [wallet, privySignTransaction],
  );

  return {
    ready,
    authenticated,
    connected: Boolean(wallet),
    owner,
    sessionKey,
    login,
    logout,
    createWallet,
    signMessage,
    signTransaction,
  };
}

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { useEmbeddedWallet } from "./useEmbeddedWallet";
import { address, getAddressEncoder } from "@solana/kit";
import {
  buildRegistrationTransaction,
  createZolanaClient,
  syncWallet,
  Wallet,
} from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";
import { connectClient } from "../lib/client";
import { walletError } from "../lib/walletError";
import {
  deriveAdapterAuthority,
  type AdapterWalletAuthority,
} from "../lib/deriveAuthority";
import { submitFactory } from "../lib/send";
import { walletAdapterSigner } from "../lib/walletAdapterSigner";
import type { Bytes32 } from "@heliuslabs/zolana/keypair";
import type { VersionedTransaction } from "@solana/web3.js";

type Client = Awaited<ReturnType<typeof createZolanaClient>>;

export type PrivateWalletContext = {
  authority: AdapterWalletAuthority;
  wallet: Wallet;
  submit: ReturnType<typeof submitFactory>;
  client: Client;
};

export type PrivateWalletStatus =
  | "disconnected"
  | "connected"
  | "initializing"
  | "signing"
  | "registering"
  | "syncing"
  | "ready"
  | "error";

export function usePrivateWallet() {
  const { owner, signMessage, signTransaction, connected, sessionKey } =
    useEmbeddedWallet();
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<PrivateWalletStatus>("disconnected");
  const [ctx, setCtx] = useState<PrivateWalletContext | null>(null);
  const session = useRef(0);
  const inFlight = useRef<number | null>(null);

  // Only an explicit initialize() call can sign. Cleanup also covers Strict Mode.
  useLayoutEffect(() => {
    session.current += 1;
    inFlight.current = null;
    setCtx(null);
    setError(null);
    setStatus(connected && owner ? "connected" : "disconnected");
    return () => {
      session.current += 1;
    };
  }, [connected, owner, sessionKey]);

  const initialize = useCallback(async () => {
    if (!connected || !owner || inFlight.current !== null || ctx) return;
    if (!signMessage || !signTransaction) {
      setError(
        "Privy message and transaction signing are unavailable. Sign in again.",
      );
      setStatus("error");
      return;
    }

    const currentSession = session.current;
    const active = () => session.current === currentSession;
    const assertActive = () => {
      if (!active())
        throw new Error(
          "Wallet changed. Connect and enable your current wallet.",
        );
    };
    inFlight.current = currentSession;
    setError(null);
    setStatus("initializing");
    try {
      const client = await connectClient();
      assertActive();
      const ownerAddress = address(owner);
      const ed25519 = Uint8Array.from(
        getAddressEncoder().encode(ownerAddress),
      ) as Bytes32;
      setStatus("signing");
      const authority = await deriveAdapterAuthority({
        solanaPublicKey: ownerAddress,
        ed25519PublicKey: ed25519,
        signMessage: async (message) => {
          assertActive();
          const signature = await signMessage(message);
          if (!active()) {
            signature.fill(0);
            assertActive();
          }
          return signature;
        },
      });
      assertActive();
      const identity = await authority.shieldedAddress();
      assertActive();
      const wallet = new Wallet({ identity });
      const signer = walletAdapterSigner({
        address: ownerAddress,
        signTransaction: async (tx) => {
          assertActive();
          const signed = await signTransaction(tx);
          assertActive();
          return signed as VersionedTransaction;
        },
      });
      const submit = submitFactory(client, signer, assertActive);
      setStatus("registering");
      const registered = await isWalletRegistered({
        rpc: client,
        owner: ownerAddress,
      });
      assertActive();
      if (!registered) {
        const registration = await buildRegistrationTransaction({
          client,
          owner: ownerAddress,
          address: identity,
        });
        assertActive();
        if (registration) await submit(registration);
        assertActive();
      }
      setStatus("syncing");
      await syncWallet({ client, wallet, authority });
      assertActive();
      setCtx({ authority, wallet, submit, client });
      setStatus("ready");
    } catch (e: unknown) {
      if (active()) {
        setError(walletError(e));
        setStatus("error");
      }
    } finally {
      if (inFlight.current === currentSession) inFlight.current = null;
    }
  }, [connected, owner, sessionKey, signMessage, signTransaction, ctx]);

  return {
    ready: status === "ready" && ctx !== null,
    status,
    error,
    ctx,
    owner,
    initialize,
  };
}

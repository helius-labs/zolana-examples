import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { address } from "@solana/kit";
import { Wallet } from "@heliuslabs/zolana";
import type { PrivateWalletContext } from "../lib/walletContext";
import { registerPrivateWallet } from "../operations/registerWallet";
import { syncPrivateWallet } from "../operations/syncWallet";
import { useEmbeddedWallet } from "./useEmbeddedWallet";
import { useBootstrapApproval } from "./useBootstrapApproval";
import { connectClient } from "../lib/client";
import { walletError } from "../lib/walletError";
import { submitFactory } from "../lib/send";
import { turnkeyTransactionSigner } from "../lib/turnkey-signer";
import { openTvcWallet } from "../lib/tvc";
import { privacyWalletTrustMaterial } from "../lib/tvc-policy";

export type PrivateWalletStatus =
  | "disconnected"
  | "connected"
  | "initializing"
  | "verifying"
  | "enrolling"
  | "signing"
  | "registering"
  | "syncing"
  | "ready"
  | "error";

export function usePrivateWallet() {
  const embedded = useEmbeddedWallet();
  const { owner, connected, sessionKey } = embedded;
  const withApproval = useBootstrapApproval(
    privacyWalletTrustMaterial().turnkeyServicePublicKey
  );
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<PrivateWalletStatus>("disconnected");
  const [ctx, setCtx] = useState<PrivateWalletContext | null>(null);
  const session = useRef<AbortController | null>(null);
  const inFlight = useRef<AbortController | null>(null);

  useLayoutEffect(() => {
    const controller = new AbortController();
    session.current = controller;
    inFlight.current = null;
    setCtx(null);
    setError(null);
    setStatus(connected && owner ? "connected" : "disconnected");
    return () => {
      controller.abort(
        new Error("Wallet session changed. Activate your current wallet.")
      );
    };
  }, [connected, owner, sessionKey]);

  const initialize = useCallback(async () => {
    const controller = session.current;
    if (
      !connected ||
      !owner ||
      !controller ||
      controller.signal.aborted ||
      inFlight.current ||
      ctx
    )
      return;
    const active = () =>
      session.current === controller && !controller.signal.aborted;
    const assertActive = () => {
      controller.signal.throwIfAborted();
      if (!active()) throw new Error("Wallet session changed.");
    };
    inFlight.current = controller;
    setError(null);
    setStatus("initializing");
    try {
      if (
        !embedded.wallet ||
        !embedded.parentOrganizationId ||
        !embedded.signTransaction
      )
        throw new Error("Turnkey wallet is unavailable. Sign in again.");
      const client = await connectClient();
      assertActive();
      const tvc = await openTvcWallet({
        wallet: embedded.wallet,
        parentOrganizationId: embedded.parentOrganizationId,
        authorize: async (profile) => {
          assertActive();
          const result = await embedded.authorizeTvcWallet(profile);
          assertActive();
          return result;
        },
        signEnrollment: async (message) => {
          assertActive();
          const result = await embedded.signTvcEnrollmentChallenge(message);
          assertActive();
          return result;
        },
        withApproval,
        signal: controller.signal,
        onStage: (stage) => {
          assertActive();
          setStatus(stage);
        },
      });
      assertActive();
      const ownerAddress = address(owner);
      const identity = tvc.keys.address();
      const wallet = new Wallet({ identity });
      const signer = turnkeyTransactionSigner(ownerAddress, async (bytes) => {
        assertActive();
        const signed = await embedded.signTransaction(bytes);
        assertActive();
        return signed;
      });
      const submit = submitFactory(client, signer, assertActive);
      const context: PrivateWalletContext = {
        owner: ownerAddress,
        keys: tvc.keys,
        wallet,
        submit,
        client,
        assertActive,
        signal: controller.signal,
      };
      setStatus("registering");
      await registerPrivateWallet(context);
      await tvc.markRegistered();
      assertActive();
      setStatus("syncing");
      await syncPrivateWallet(context);
      assertActive();
      setCtx(context);
      setStatus("ready");
    } catch (e) {
      if (active()) {
        setError(walletError(e));
        setStatus("error");
      }
    } finally {
      if (inFlight.current === controller) inFlight.current = null;
    }
  }, [connected, owner, sessionKey, embedded, withApproval, ctx]);

  return {
    ready: status === "ready" && ctx !== null,
    status,
    error,
    ctx,
    owner,
    initialize,
  };
}

"use client";

import { AuthState, useTurnkey } from "@turnkey/react-wallet-kit";
import { useCallback, useLayoutEffect, useMemo, useRef } from "react";
import { bootstrapWithApproval } from "../lib/bootstrap-approval";

/** Uses the embedded wallet's owner session; the app never receives an approval result. */
export function useBootstrapApproval(servicePublicKey: string | null) {
  const { authState, httpClient, session, wallets } = useTurnkey();
  const organizationId = session?.organizationId;
  const userId = session?.userId;
  const sessionToken = session?.token;
  const walletId = wallets?.[0]?.walletId;
  const accountId = wallets?.[0]?.accounts?.[0]?.walletAccountId;
  const walletAddress = wallets?.[0]?.accounts?.[0]?.address;
  const binding = useMemo(
    () => ({
      authState,
      httpClient,
      organizationId,
      userId,
      sessionToken,
      walletId,
      accountId,
      walletAddress,
      servicePublicKey,
    }),
    [
      authState,
      httpClient,
      organizationId,
      userId,
      sessionToken,
      walletId,
      accountId,
      walletAddress,
      servicePublicKey,
    ],
  );
  const active = useRef<{
    binding: typeof binding;
    controller: AbortController;
    pending: boolean;
  } | null>(null);

  useLayoutEffect(() => {
    const scope = {
      binding,
      controller: new AbortController(),
      pending: false,
    };
    active.current = scope;
    return () => {
      scope.controller.abort(new Error("TvcBootstrapSessionChanged"));
      if (active.current === scope) active.current = null;
    };
  }, [binding]);

  return useCallback(
    async <T>(
      expectedWalletAddress: string,
      start: (signal: AbortSignal) => Promise<T>,
      callerSignal?: AbortSignal,
    ): Promise<T> => {
      const scope = active.current;
      if (!scope || scope.binding !== binding)
        throw new Error("TvcBootstrapSessionChanged");
      if (
        binding.authState !== AuthState.Authenticated ||
        !binding.httpClient ||
        !binding.organizationId ||
        !binding.userId ||
        !binding.servicePublicKey
      )
        throw new Error("HeliusWalletSessionRequired");
      if (expectedWalletAddress !== binding.walletAddress)
        throw new Error("WalletBindingMismatch");
      if (scope.pending) throw new Error("TvcBootstrapAlreadyPending");
      scope.pending = true;
      try {
        return await bootstrapWithApproval(
          binding.httpClient,
          {
            organizationId: binding.organizationId,
            walletAddress: expectedWalletAddress,
            servicePublicKey: binding.servicePublicKey,
          },
          start,
          AbortSignal.any([
            scope.controller.signal,
            ...(callerSignal ? [callerSignal] : []),
          ]),
        );
      } finally {
        scope.pending = false;
      }
    },
    [binding],
  );
}

import { useEffect, useState } from "react";
import { useWallet } from "@solana/wallet-adapter-react";
import { address, getAddressEncoder } from "@solana/kit";
import {
  buildRegistrationTransaction,
  createZolanaClient,
  syncWallet,
  Wallet,
} from "@heliuslabs/zolana";
import { isWalletRegistered } from "@heliuslabs/zolana/wallet";
import { connectClient } from "../lib/client";
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

export function usePrivateWallet() {
  const { publicKey, signMessage, signTransaction, connected } = useWallet();
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [ctx, setCtx] = useState<PrivateWalletContext | null>(null);

  useEffect(() => {
    if (!connected || !publicKey || !signMessage || !signTransaction) {
      setCtx(null);
      setReady(false);
      return;
    }
    let cancelled = false;
    (async () => {
      const client = await connectClient();
      const owner = address(publicKey.toBase58());
      const ed25519 = Uint8Array.from(
        getAddressEncoder().encode(owner),
      ) as Bytes32;
      const authority = await deriveAdapterAuthority({
        solanaPublicKey: owner,
        ed25519PublicKey: ed25519,
        signMessage: (message) => signMessage(message),
      });
      const wallet = new Wallet({
        identity: await authority.shieldedAddress(),
      });
      const signer = walletAdapterSigner({
        address: owner,
        signTransaction: async (tx) => {
          const signed = await signTransaction(tx);
          return signed as VersionedTransaction;
        },
      });
      const submit = submitFactory(client, signer);
      if (!(await isWalletRegistered({ rpc: client, owner }))) {
        const registration = await buildRegistrationTransaction({
          client,
          owner,
          address: await authority.shieldedAddress(),
        });
        if (registration) await submit(registration);
      }
      await syncWallet({ client, wallet, authority });
      if (!cancelled) {
        setCtx({ authority, wallet, submit, client });
        setReady(true);
      }
    })().catch((e: unknown) => {
      if (!cancelled) setError(e instanceof Error ? e.message : String(e));
    });
    return () => {
      cancelled = true;
    };
  }, [connected, publicKey, signMessage, signTransaction]);

  return { ready, error, ctx, owner: publicKey?.toBase58() ?? "" };
}

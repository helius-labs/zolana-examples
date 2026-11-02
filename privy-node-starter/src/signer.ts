import {
  address,
  getAddressEncoder,
  getTransactionDecoder,
  getTransactionEncoder,
  type TransactionPartialSigner,
} from "@solana/kit";
import { LocalKeys, type ProofService } from "@heliuslabs/zolana/client";
import { ed25519DerivationMessage, type Bytes32 } from "@heliuslabs/zolana/keypair";

export interface RemoteWallet {
  address: string;
  signMessage(message: Uint8Array): Promise<Uint8Array>;
  signTransaction(transaction: Uint8Array): Promise<Uint8Array>;
}

// Same Kit wire-transaction adapter as embedded-private-wallet's walletAdapterSigner.ts.
export function remoteSigner(wallet: RemoteWallet): TransactionPartialSigner {
  const owner = address(wallet.address);
  return {
    address: owner,
    async signTransactions(transactions) {
      const signatures = [];
      for (const transaction of transactions) {
        const signed = getTransactionDecoder().decode(
          await wallet.signTransaction(new Uint8Array(getTransactionEncoder().encode(transaction))),
        );
        if (!equalBytes(transaction.messageBytes, signed.messageBytes)) {
          throw new Error("Privy changed the transaction message");
        }
        const signature = signed.signatures[owner];
        if (!signature || signature.every((byte) => byte === 0)) {
          throw new Error("Privy did not sign for the requested wallet");
        }
        signatures.push({ [owner]: signature });
      }
      return signatures;
    },
  };
}

export function equalBytes(a: ArrayLike<number>, b: ArrayLike<number>): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export async function activate(wallet: RemoteWallet, proofs: ProofService) {
  const owner = address(wallet.address);
  const publicKey = new Uint8Array(getAddressEncoder().encode(owner)) as Bytes32;
  const message = ed25519DerivationMessage(publicKey);
  const seed = await wallet.signMessage(message);
  try {
    // The SDK verifies the Ed25519 signature before deriving privacy keys.
    return LocalKeys.fromDerivationSeed({ solanaPublicKey: owner, derivationSeed: seed }, proofs);
  } finally {
    seed.fill(0);
  }
}

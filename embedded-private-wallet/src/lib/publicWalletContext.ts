import { address } from "@solana/kit";
import { connectClient } from "./client";
import { submitFactory } from "./send";
import {
  turnkeyTransactionSigner,
  type SignSerializedTransaction,
} from "./turnkey-signer";
import type { PublicWalletContext } from "./walletContext";

/** Public sends need the connected Turnkey session, without TVC activation. */
export async function createPublicWalletContext(
  owner: string,
  signTransaction: SignSerializedTransaction,
  signal: AbortSignal,
  assertActive: () => void,
): Promise<PublicWalletContext> {
  assertActive();
  const client = await connectClient();
  assertActive();
  const signer = turnkeyTransactionSigner(owner, async (bytes) => {
    assertActive();
    const signed = await signTransaction(bytes);
    assertActive();
    return signed;
  });
  return {
    owner: address(owner),
    client,
    signal,
    assertActive,
    submit: submitFactory(client, signer, assertActive),
  };
}
